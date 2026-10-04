//! Which agents a window has seen, kept across detach and reattach.
//!
//! A new window normally counts every agent that is already waiting as seen. This keeps each
//! window's record per server boot in a file next to its chrome preferences, so reattaching to
//! the same servers picks up where the window left off. A restarted server has a new boot id
//! and starts fresh.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Records older than this belong to servers that are long gone.
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct SavedAgentSeen {
    #[serde(default)]
    pub(super) acknowledged: HashMap<String, u64>,
    #[serde(default)]
    pub(super) completed: HashMap<String, u64>,
    #[serde(default)]
    pub(super) working: HashSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SavedBoot {
    saved_at: u64,
    #[serde(flatten)]
    seen: SavedAgentSeen,
}

#[derive(Debug, Default)]
pub(super) struct AgentSeenStore {
    /// Records by server boot id, read from disk on first use.
    boots: Option<BTreeMap<String, SavedBoot>>,
}

impl AgentSeenStore {
    fn boots(&mut self, path: &Path) -> &mut BTreeMap<String, SavedBoot> {
        self.boots.get_or_insert_with(|| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        })
    }

    fn get(&mut self, path: &Path, boot_id: &str) -> Option<SavedAgentSeen> {
        self.boots(path).get(boot_id).map(|boot| boot.seen.clone())
    }

    fn save<'a>(
        &mut self,
        path: &Path,
        current: impl IntoIterator<Item = (&'a str, SavedAgentSeen)>,
    ) {
        let now = unix_now();
        let boots = self.boots(path);
        for (boot_id, seen) in current {
            boots.insert(
                boot_id.to_owned(),
                SavedBoot {
                    saved_at: now,
                    seen,
                },
            );
        }
        boots.retain(|_, boot| now.saturating_sub(boot.saved_at) <= MAX_AGE_SECS);
        if let Err(error) = super::preferences::store(path, &*boots) {
            tracing::warn!(%error, "failed to save which agents this window has seen");
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl super::ClientShellState {
    fn agent_seen_path(&self) -> Option<PathBuf> {
        self.config
            .preferences_path
            .as_deref()
            .map(|preferences| preferences.with_extension("seen.json"))
    }

    /// The first snapshot of a server boot this window saw before picks up where it left off.
    pub(super) fn restore_agent_seen(&mut self, endpoint_index: usize, boot_id: &str) {
        let Some(path) = self.agent_seen_path() else {
            return;
        };
        if self.endpoints[endpoint_index].agent_presentation.boot_id() == Some(boot_id) {
            return;
        }
        if let Some(saved) = self.agent_seen.get(&path, boot_id) {
            self.endpoints[endpoint_index]
                .agent_presentation
                .restore_seen(boot_id.to_owned(), saved);
        }
    }

    pub(super) fn persist_agent_seen(&mut self) {
        let Some(path) = self.agent_seen_path() else {
            return;
        };
        let current = self.endpoints.iter().filter_map(|endpoint| {
            let presentation = &endpoint.agent_presentation;
            Some((presentation.boot_id()?, presentation.saved_seen()))
        });
        self.agent_seen.save(&path, current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_a_new_store_and_old_ones_are_dropped() {
        let path =
            std::env::temp_dir().join(format!("herdr-agent-seen-{}.seen.json", std::process::id()));
        let stale = BTreeMap::from([(
            "old-boot".to_owned(),
            SavedBoot {
                saved_at: unix_now() - MAX_AGE_SECS - 1,
                seen: SavedAgentSeen::default(),
            },
        )]);
        super::super::preferences::store(&path, &stale).expect("write stale record");

        let seen = SavedAgentSeen {
            acknowledged: HashMap::from([("pane".to_owned(), 4)]),
            completed: HashMap::from([("pane".to_owned(), 6)]),
            working: HashSet::from(["other".to_owned()]),
        };
        AgentSeenStore::default().save(&path, [("boot", seen.clone())]);

        let mut reloaded = AgentSeenStore::default();
        assert_eq!(reloaded.get(&path, "boot"), Some(seen));
        assert_eq!(reloaded.get(&path, "old-boot"), None);
        std::fs::remove_file(path).expect("remove record");
    }
}
