//! Fork: the window's focus history behind the `last_tab` and `last_workspace` keys.

use super::*;
use crate::input::KeybindAction;

/// Enough history to skip over closed tabs and spaces without growing forever.
const RECENT_FOCUS_LIMIT: usize = 64;

/// How long a space asked for from another session's + may take to open.
const WORKSPACE_CREATE_HOLD: std::time::Duration = std::time::Duration::from_secs(10);

/// The tabs the window has shown, most recent first.
#[derive(Default)]
pub(super) struct RecentFocusHistory {
    entries: Vec<RecentFocus>,
    hold: Option<FocusHold>,
}

/// A space being created in another session. Switching there first shows that session's old
/// space, which doesn't count as visited: the history from before it comes back once the new
/// space opens.
struct FocusHold {
    endpoint_id: ClientEndpointId,
    deadline: std::time::Instant,
    before: Option<Vec<RecentFocus>>,
}

/// A tab the window has shown. The boot ID keeps a restarted session's reused IDs apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RecentFocus {
    endpoint_id: ClientEndpointId,
    boot_id: String,
    workspace_id: String,
    tab_id: String,
}

impl RecentFocus {
    fn same_workspace(&self, other: &Self) -> bool {
        self.endpoint_id == other.endpoint_id
            && self.boot_id == other.boot_id
            && self.workspace_id == other.workspace_id
    }
}

impl ClientShellState {
    fn current_focus(&self) -> Option<RecentFocus> {
        let snapshot = self.snapshot.as_deref()?;
        Some(RecentFocus {
            endpoint_id: self.active_endpoint_id.clone(),
            boot_id: snapshot.boot_id.clone(),
            workspace_id: snapshot.focused_workspace_id.clone()?,
            tab_id: snapshot.focused_tab_id.clone()?,
        })
    }

    /// Moves the focused tab to the front of the history, most recent first.
    pub(super) fn note_recent_focus(&mut self) {
        let Some(focus) = self.current_focus() else {
            return;
        };
        let history = &mut self.recent_focus;
        if history.entries.first() == Some(&focus) {
            return;
        }
        if let Some(hold) = history.hold.take() {
            if hold.endpoint_id == focus.endpoint_id && std::time::Instant::now() < hold.deadline {
                match hold.before {
                    None => {
                        history.hold = Some(FocusHold {
                            before: Some(history.entries.clone()),
                            ..hold
                        });
                    }
                    Some(before) => {
                        if history
                            .entries
                            .first()
                            .is_some_and(|shown| !shown.same_workspace(&focus))
                        {
                            history.entries = before;
                        }
                    }
                }
            }
        }
        history.entries.retain(|recent| recent != &focus);
        history.entries.insert(0, focus);
        history.entries.truncate(RECENT_FOCUS_LIMIT);
    }

    pub(super) fn hold_recent_focus(&mut self, endpoint_id: ClientEndpointId) {
        self.recent_focus.hold = Some(FocusHold {
            endpoint_id,
            deadline: std::time::Instant::now() + WORKSPACE_CREATE_HOLD,
            before: None,
        });
    }

    /// `last_tab` goes back to the tab shown before this one in the same space; `last_workspace`
    /// to the space shown before this one, in any session the sidebar lists. Pressing either
    /// again comes back. Closed tabs and spaces and sessions not online are skipped.
    pub(super) fn handle_recent_focus(
        &mut self,
        action: KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(
            action,
            KeybindAction::LastTab | KeybindAction::LastWorkspace
        ) {
            return false;
        }
        let current = self.current_focus();
        let last_tab = action == KeybindAction::LastTab;
        let Some(target) = self
            .recent_focus
            .entries
            .iter()
            .filter(|recent| match &current {
                Some(current) if last_tab => {
                    recent.same_workspace(current) && recent.tab_id != current.tab_id
                }
                Some(current) => !recent.same_workspace(current),
                // Nothing on screen, say after a session's last space closed: any space will do.
                None => !last_tab,
            })
            .find(|recent| self.recent_focus_exists(recent, last_tab))
            .cloned()
        else {
            return true;
        };
        if last_tab {
            self.push_endpoint_method(
                crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget {
                    tab_id: target.tab_id,
                }),
                outcome,
            );
        } else {
            if !self.lists_sessions() {
                self.reveal_workspace(&target.workspace_id);
            }
            self.focus_or_activate(
                target.endpoint_id,
                ClientEndpointFocusTarget::Workspace(target.workspace_id),
                outcome,
            );
        }
        true
    }

    fn recent_focus_exists(&self, recent: &RecentFocus, tab: bool) -> bool {
        let snapshot = if recent.endpoint_id == self.active_endpoint_id {
            self.snapshot.as_deref()
        } else {
            self.endpoints
                .iter()
                .find(|endpoint| {
                    endpoint.endpoint_id == recent.endpoint_id
                        && endpoint.status == ClientEndpointStatus::Online
                })
                .and_then(|endpoint| endpoint.snapshot.as_deref())
        };
        snapshot.is_some_and(|snapshot| {
            snapshot.boot_id == recent.boot_id
                && if tab {
                    snapshot.tabs.iter().any(|candidate| {
                        candidate.workspace_id == recent.workspace_id
                            && candidate.tab_id == recent.tab_id
                    })
                } else {
                    snapshot
                        .workspaces
                        .iter()
                        .any(|candidate| candidate.workspace_id == recent.workspace_id)
                }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::snapshot;
    use super::*;
    use crate::api::schema::Method;
    use crate::client::endpoint::{ProfileId, SavedSshEndpoint};
    use crate::config::Config;

    /// Spaces ws_1 (tabs ws_1:t1, ws_1:t2, ws_1:t3) and ws_2 (ws_2:t1, ws_2:t2).
    fn focused(revision: u64, workspace_id: &str, tab_id: &str) -> Box<ClientShellSnapshot> {
        let mut snapshot = snapshot();
        let workspace = snapshot.workspaces[0].clone();
        let tab = snapshot.tabs[0].clone();
        snapshot.revision = revision;
        snapshot.workspaces = ["ws_1", "ws_2"]
            .into_iter()
            .map(|id| ClientShellWorkspace {
                workspace_id: id.into(),
                focused: id == workspace_id,
                ..workspace.clone()
            })
            .collect();
        snapshot.tabs = [("ws_1", 3), ("ws_2", 2)]
            .into_iter()
            .flat_map(|(workspace_id, count)| (1..=count).map(move |n| (workspace_id, n)))
            .map(|(workspace_id, n)| crate::protocol::ClientShellTab {
                tab_id: format!("{workspace_id}:t{n}"),
                workspace_id: workspace_id.into(),
                number: n,
                ..tab.clone()
            })
            .collect();
        snapshot.focused_workspace_id = Some(workspace_id.into());
        snapshot.focused_tab_id = Some(tab_id.into());
        Box::new(snapshot)
    }

    fn press(state: &mut ClientShellState, action: KeybindAction) -> Vec<ClientShellAction> {
        let mut outcome = ClientShellInput::default();
        state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
        outcome.actions
    }

    fn focused_tab(actions: &[ClientShellAction]) -> &str {
        match actions {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                Method::TabFocus(target) => &target.tab_id,
                method => panic!("expected a tab focus, got {method:?}"),
            },
            actions => panic!("expected one endpoint request, got {actions:?}"),
        }
    }

    fn focused_workspace(actions: &[ClientShellAction]) -> &str {
        match actions {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                Method::WorkspaceFocus(target) => &target.workspace_id,
                method => panic!("expected a workspace focus, got {method:?}"),
            },
            actions => panic!("expected one endpoint request, got {actions:?}"),
        }
    }

    #[test]
    fn last_tab_and_last_workspace_toggle_between_the_two_latest() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        assert!(press(&mut state, KeybindAction::LastTab).is_empty());

        let visits = [
            ("ws_1", "ws_1:t1"),
            ("ws_1", "ws_1:t2"),
            ("ws_2", "ws_2:t1"),
            ("ws_2", "ws_2:t2"),
            ("ws_1", "ws_1:t2"),
        ];
        for (revision, (workspace_id, tab_id)) in (1..).zip(visits) {
            state.set_snapshot(focused(revision, workspace_id, tab_id));
        }

        // Each space remembers its own previous tab.
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_2"
        );

        state.set_snapshot(focused(6, "ws_1", "ws_1:t1"));
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t2"
        );

        state.set_snapshot(focused(7, "ws_2", "ws_2:t2"));
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_2:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_1"
        );
    }

    #[test]
    fn closed_tabs_and_spaces_are_skipped() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(focused(1, "ws_1", "ws_1:t1"));
        state.set_snapshot(focused(2, "ws_1", "ws_1:t2"));
        state.set_snapshot(focused(3, "ws_1", "ws_1:t3"));
        let mut closed = focused(4, "ws_1", "ws_1:t3");
        closed.tabs.retain(|tab| tab.tab_id != "ws_1:t2");
        state.set_snapshot(closed);
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t1"
        );

        state.set_snapshot(focused(5, "ws_2", "ws_2:t1"));
        let mut closed = focused(6, "ws_1", "ws_1:t1");
        closed
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws_2");
        closed.tabs.retain(|tab| tab.workspace_id != "ws_2");
        state.set_snapshot(closed);
        assert!(press(&mut state, KeybindAction::LastWorkspace).is_empty());

        // A restarted session reuses IDs for different tabs.
        let mut rebooted = focused(1, "ws_1", "ws_1:t3");
        rebooted.boot_id = "boot-2".into();
        state.set_snapshot(rebooted);
        assert!(press(&mut state, KeybindAction::LastTab).is_empty());
    }

    #[test]
    fn last_workspace_switches_back_to_another_session() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let profile = SavedSshEndpoint {
            id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            label: "Build".into(),
            target: "dev@build.example".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        state.set_snapshot(focused(1, "ws_1", "ws_1:t1"));
        let mut remote_snapshot = focused(1, "ws_2", "ws_2:t1");
        remote_snapshot.boot_id = "remote-boot".into();
        state.set_endpoint_snapshot(&remote, remote_snapshot);
        assert!(state.activate_endpoint_projection(&remote));

        let actions = press(&mut state, KeybindAction::LastWorkspace);
        assert!(
            matches!(
                actions.as_slice(),
                [ClientShellAction::ActivateEndpoint {
                    endpoint_id: ClientEndpointId::Local,
                    target: Some(ClientEndpointFocusTarget::Workspace(workspace_id)),
                }] if workspace_id == "ws_1"
            ),
            "{actions:?}"
        );

        // With no space left on screen, any remembered space will do.
        let mut empty = focused(2, "ws_2", "ws_2:t1");
        empty.boot_id = "remote-boot".into();
        empty.workspaces.clear();
        empty.tabs.clear();
        empty.focused_workspace_id = None;
        empty.focused_tab_id = None;
        state.set_endpoint_snapshot(&remote, empty);
        assert!(press(&mut state, KeybindAction::LastTab).is_empty());
        let actions = press(&mut state, KeybindAction::LastWorkspace);
        assert!(
            matches!(
                actions.as_slice(),
                [ClientShellAction::ActivateEndpoint {
                    endpoint_id: ClientEndpointId::Local,
                    target: Some(ClientEndpointFocusTarget::Workspace(workspace_id)),
                }] if workspace_id == "ws_1"
            ),
            "{actions:?}"
        );
    }
}
