use std::collections::{HashMap, HashSet};

use crate::api::schema::AgentStatus;
use crate::protocol::{ClientShellAgent, ClientShellSnapshot, PaneSurfaceFrame};

#[derive(Clone, Debug, Default)]
pub(super) struct EndpointAgentPresentation {
    boot_id: Option<String>,
    acknowledged: HashMap<String, u64>,
    completed: HashMap<String, u64>,
    working: HashSet<String>,
    pending_completions: Option<(
        Option<u64>,
        crate::protocol::endpoint::EndpointAgentCompletions,
    )>,
    /// Fork: a pane counts as looked at once it has been on screen this long (`ui.seen_delay_ms`).
    pub(super) seen_delay: std::time::Duration,
    /// Fork: when each pane on screen came into view.
    on_screen_since: HashMap<String, std::time::Instant>,
    /// Fork: panes whose agent the server reports idle rather than done, so it counts a finish
    /// there as seen. A pane on screen follows it, so the window and the server agree.
    server_seen: HashSet<String>,
}

impl EndpointAgentPresentation {
    pub(super) fn receive_completions(
        &mut self,
        generation: Option<u64>,
        completions: crate::protocol::endpoint::EndpointAgentCompletions,
    ) {
        if self
            .pending_completions
            .as_ref()
            .is_some_and(|(current_generation, current)| {
                *current_generation == generation
                    && current.boot_id == completions.boot_id
                    && current.revision >= completions.revision
            })
        {
            return;
        }
        self.pending_completions = Some((generation, completions));
    }

    #[cfg(test)]
    pub(super) fn project_snapshot(&mut self, snapshot: &mut ClientShellSnapshot) {
        self.project_snapshot_for_generation(snapshot, None);
    }

    pub(super) fn project_snapshot_for_generation(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        generation: Option<u64>,
    ) {
        if self.boot_id.as_deref() != Some(snapshot.boot_id.as_str()) {
            self.boot_id = Some(snapshot.boot_id.clone());
            self.acknowledged.clear();
            self.completed.clear();
            self.working.clear();
            // What happened before this window was watching is the server's call: agents it
            // still reports done, and open questions, wait until their pane is on screen.
            for agent in &snapshot.agents {
                match agent.agent_status {
                    AgentStatus::Done => {
                        self.completed
                            .insert(agent.pane_id.clone(), agent.state_change_seq);
                    }
                    AgentStatus::Blocked => {}
                    _ => {
                        self.acknowledged
                            .insert(agent.pane_id.clone(), agent.state_change_seq);
                    }
                }
            }
        }
        let pane_ids: HashSet<&str> = snapshot
            .agents
            .iter()
            .map(|agent| agent.pane_id.as_str())
            .collect();
        self.acknowledged
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.completed
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.working
            .retain(|pane_id| pane_ids.contains(pane_id.as_str()));
        let completions = self
            .pending_completions
            .take()
            .filter(|(received_generation, projection)| {
                *received_generation == generation
                    && projection.boot_id == snapshot.boot_id
                    && projection.revision == snapshot.revision
            })
            .map(|(_, projection)| projection.completions);
        self.server_seen = snapshot
            .agents
            .iter()
            .filter(|agent| agent.agent_status == AgentStatus::Idle)
            .map(|agent| agent.pane_id.clone())
            .collect();
        for agent in &mut snapshot.agents {
            match agent.agent_status {
                AgentStatus::Working => {
                    self.working.insert(agent.pane_id.clone());
                    self.completed.remove(&agent.pane_id);
                }
                AgentStatus::Blocked => {
                    self.completed.remove(&agent.pane_id);
                }
                AgentStatus::Idle | AgentStatus::Done => {
                    let observed_work = self.working.remove(&agent.pane_id);
                    let completed = completions.as_ref().map_or_else(
                        || {
                            observed_work
                                || self.completed.get(&agent.pane_id)
                                    == Some(&agent.state_change_seq)
                        },
                        |completions| {
                            completions.get(&agent.pane_id) == Some(&agent.state_change_seq)
                        },
                    );
                    if completed {
                        self.completed
                            .insert(agent.pane_id.clone(), agent.state_change_seq);
                    } else {
                        self.completed.remove(&agent.pane_id);
                    }
                }
                _ => {
                    self.working.remove(&agent.pane_id);
                    self.completed.remove(&agent.pane_id);
                }
            }
            agent.agent_status = self.projected_status(agent);
        }
        project_aggregate_status(snapshot);
    }

    pub(super) fn acknowledge_surface(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        surface: &PaneSurfaceFrame,
        outer_focused: Option<bool>,
    ) -> bool {
        if outer_focused == Some(false) {
            self.leave_screen();
            return false;
        }
        // Fork: a pane that left the screen starts its wait over when it comes back.
        self.on_screen_since
            .retain(|pane_id, _| surface.panes.iter().any(|pane| &pane.pane_id == pane_id));
        if self.boot_id.as_deref() != Some(surface.boot_id.as_str())
            || snapshot.boot_id != surface.boot_id
            || snapshot.revision != surface.projection_revision
        {
            return false;
        }

        let now = std::time::Instant::now();
        let mut changed = false;
        for pane in &surface.panes {
            let since = match self.on_screen_since.get(&pane.pane_id) {
                Some(since) => *since,
                None => {
                    self.on_screen_since.insert(pane.pane_id.clone(), now);
                    now
                }
            };
            if now.saturating_duration_since(since) < self.seen_delay
                && !self.server_seen.contains(&pane.pane_id)
            {
                continue;
            }
            let Some(agent) = snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane.pane_id)
            else {
                continue;
            };
            let acknowledged = self.acknowledged.entry(agent.pane_id.clone()).or_default();
            if *acknowledged < agent.state_change_seq {
                *acknowledged = agent.state_change_seq;
                changed = true;
            }
        }
        if changed {
            for agent in &mut snapshot.agents {
                agent.agent_status = self.projected_status(agent);
            }
            project_aggregate_status(snapshot);
        }
        changed
    }

    /// Fork: nothing of this endpoint is being looked at, such as when the window is in the
    /// background or shows another session, so every pane's wait starts over.
    pub(super) fn leave_screen(&mut self) {
        self.on_screen_since.clear();
    }

    pub(super) fn seen(&self, agent: &ClientShellAgent) -> bool {
        self.completed.get(&agent.pane_id).is_none_or(|completion| {
            self.acknowledged
                .get(&agent.pane_id)
                .is_some_and(|sequence| sequence >= completion)
        })
    }

    /// Finished or asking a question, and its pane has not been on screen since.
    pub(super) fn awaits_user(&self, agent: &ClientShellAgent) -> bool {
        match agent.agent_status {
            AgentStatus::Done => true,
            AgentStatus::Blocked => self
                .acknowledged
                .get(&agent.pane_id)
                .is_none_or(|sequence| *sequence < agent.state_change_seq),
            _ => false,
        }
    }

    fn projected_status(&self, agent: &ClientShellAgent) -> AgentStatus {
        match agent.agent_status {
            AgentStatus::Idle | AgentStatus::Done => {
                if self.seen(agent) {
                    AgentStatus::Idle
                } else {
                    AgentStatus::Done
                }
            }
            status => status,
        }
    }
}

fn project_aggregate_status(snapshot: &mut ClientShellSnapshot) {
    for tab in &mut snapshot.tabs {
        if let Some(status) = snapshot
            .agents
            .iter()
            .filter(|agent| agent.tab_id == tab.tab_id)
            .map(|agent| agent.agent_status)
            .max_by_key(|status| super::status_priority(*status))
        {
            tab.agent_status = status;
        }
    }
    for workspace in &mut snapshot.workspaces {
        if let Some(status) = snapshot
            .agents
            .iter()
            .filter(|agent| agent.workspace_id == workspace.workspace_id)
            .map(|agent| agent.agent_status)
            .max_by_key(|status| super::status_priority(*status))
        {
            workspace.agent_status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        endpoint::EndpointAgentCompletions, FrameData, PaneSurfacePane, SurfaceRect,
    };

    fn agent(status: AgentStatus, sequence: u64) -> ClientShellAgent {
        ClientShellAgent {
            pane_id: "agent-pane".into(),
            workspace_id: "workspace".into(),
            tab_id: "tab".into(),
            name: None,
            display_agent: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: sequence,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        }
    }

    fn snapshot(status: AgentStatus, sequence: u64, revision: u64) -> ClientShellSnapshot {
        let mut snapshot = crate::client::shell::tests::snapshot();
        snapshot.boot_id = "endpoint-boot".into();
        snapshot.revision = revision;
        snapshot.agents = vec![agent(status, sequence)];
        snapshot
    }

    fn surface(revision: u64) -> PaneSurfaceFrame {
        let rect = SurfaceRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        PaneSurfaceFrame {
            boot_id: "endpoint-boot".into(),
            projection_revision: revision,
            surface_revision: 1,
            frame: FrameData {
                cells: Vec::new(),
                width: 0,
                height: 0,
                cursor: None,
                hyperlinks: Vec::new(),
                graphics: Vec::new(),
            },
            panes: vec![PaneSurfacePane {
                pane_id: "agent-pane".into(),
                content_revision: 1,
                rect,
                inner_rect: rect,
                scrollbar_rect: None,
                scroll: None,
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: Default::default(),
        }
    }

    #[test]
    fn first_snapshot_keeps_what_the_server_reports_done() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut snapshot = snapshot(AgentStatus::Done, 4, 1);

        presentation.project_snapshot(&mut snapshot);

        assert_eq!(snapshot.agents[0].agent_status, AgentStatus::Done);
        assert!(presentation.awaits_user(&snapshot.agents[0]));
    }

    #[test]
    fn first_snapshot_takes_idle_agents_as_seen() {
        let mut presentation = EndpointAgentPresentation::default();
        presentation.receive_completions(None, completions("endpoint-boot", 1, Some(4)));
        let mut snapshot = snapshot(AgentStatus::Idle, 4, 1);

        presentation.project_snapshot(&mut snapshot);

        assert_eq!(snapshot.agents[0].agent_status, AgentStatus::Idle);
        assert!(!presentation.awaits_user(&snapshot.agents[0]));
    }

    fn assert_idle_sequence(states: &[(AgentStatus, u64)]) {
        let mut presentation = EndpointAgentPresentation::default();
        let mut projected = AgentStatus::Unknown;
        for (revision, &(status, seq)) in states.iter().enumerate() {
            let mut snapshot = snapshot(status, seq, revision as u64 + 1);
            presentation.project_snapshot(&mut snapshot);
            projected = snapshot.agents[0].agent_status;
        }
        assert_eq!(projected, AgentStatus::Idle, "{states:?}");
    }

    fn completions(boot: &str, revision: u64, seq: Option<u64>) -> EndpointAgentCompletions {
        EndpointAgentCompletions {
            boot_id: boot.into(),
            revision,
            completions: seq
                .map(|seq| ("agent-pane".into(), seq))
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn completion_guard_initial_unknown_to_idle_does_not_project_done() {
        assert_idle_sequence(&[(AgentStatus::Unknown, 0), (AgentStatus::Idle, 1)]);
    }

    #[test]
    fn completion_guard_new_idle_agent_is_not_completed_work() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut baseline = snapshot(AgentStatus::Idle, 0, 1);
        baseline.agents.clear();
        presentation.project_snapshot(&mut baseline);
        let mut ready = snapshot(AgentStatus::Idle, 1, 2);

        presentation.project_snapshot(&mut ready);

        assert_eq!(ready.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn completion_guard_session_rebind_does_not_project_done() {
        assert_idle_sequence(&[
            (AgentStatus::Idle, 4),
            (AgentStatus::Unknown, 5),
            (AgentStatus::Idle, 6),
        ]);
    }

    #[test]
    fn completion_guard_startup_blocker_does_not_project_done() {
        assert_idle_sequence(&[
            (AgentStatus::Unknown, 0),
            (AgentStatus::Blocked, 1),
            (AgentStatus::Idle, 2),
        ]);
    }

    #[test]
    fn completion_guard_distinguishes_coalesced_work_from_rebind() {
        for completion in [None, Some(6)] {
            let mut presentation = EndpointAgentPresentation::default();
            let mut baseline = snapshot(AgentStatus::Idle, 4, 1);
            presentation.project_snapshot(&mut baseline);
            presentation.receive_completions(None, completions(&baseline.boot_id, 2, completion));
            let mut settled = snapshot(AgentStatus::Idle, 6, 2);

            presentation.project_snapshot(&mut settled);

            let expected = if completion.is_some() {
                AgentStatus::Done
            } else {
                AgentStatus::Idle
            };
            assert_eq!(settled.agents[0].agent_status, expected);
            presentation.project_snapshot(&mut settled);
            assert_eq!(settled.agents[0].agent_status, expected);
        }
    }

    #[test]
    fn completion_guard_server_suppression_overrides_client_observed_work() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut baseline = snapshot(AgentStatus::Working, 1, 1);
        presentation.project_snapshot(&mut baseline);
        presentation.receive_completions(None, completions(&baseline.boot_id, 2, None));
        let mut settled = snapshot(AgentStatus::Idle, 2, 2);

        presentation.project_snapshot(&mut settled);

        assert_eq!(settled.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn completion_guard_companion_is_scoped_to_boot_revision_and_connection() {
        for (boot, revision, generation, seq) in [
            ("old-boot", 2, Some(1), 6),
            ("endpoint-boot", 1, Some(1), 6),
            ("endpoint-boot", 2, Some(0), 6),
            ("endpoint-boot", 2, Some(1), 5),
        ] {
            let mut presentation = EndpointAgentPresentation::default();
            let mut baseline = snapshot(AgentStatus::Idle, 4, 1);
            presentation.project_snapshot_for_generation(&mut baseline, Some(1));
            presentation.receive_completions(generation, completions(boot, revision, Some(seq)));
            let mut settled = snapshot(AgentStatus::Idle, 6, 2);

            presentation.project_snapshot_for_generation(&mut settled, Some(1));

            assert_eq!(settled.agents[0].agent_status, AgentStatus::Idle);
        }
    }

    #[test]
    fn completion_guard_new_boot_baselines_existing_completions() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut old = snapshot(AgentStatus::Working, 1, 1);
        old.boot_id = "old-boot".into();
        presentation.project_snapshot(&mut old);
        presentation.receive_completions(None, completions("endpoint-boot", 1, Some(6)));
        let mut restored = snapshot(AgentStatus::Idle, 6, 1);

        presentation.project_snapshot(&mut restored);

        assert_eq!(restored.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn unpresented_working_completion_projects_done() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);

        presentation.project_snapshot(&mut completed);

        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn coherent_presented_surface_acknowledges_completion() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);

        assert!(presentation.acknowledge_surface(&mut completed, &surface(2), Some(true)));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn clients_acknowledge_the_same_endpoint_completion_independently() {
        let mut viewing_client = EndpointAgentPresentation::default();
        let mut background_client = EndpointAgentPresentation::default();
        let mut initial_for_viewer = snapshot(AgentStatus::Working, 4, 1);
        let mut initial_for_background = initial_for_viewer.clone();
        viewing_client.project_snapshot(&mut initial_for_viewer);
        background_client.project_snapshot(&mut initial_for_background);
        let mut completed_for_viewer = snapshot(AgentStatus::Idle, 5, 2);
        let mut completed_for_background = completed_for_viewer.clone();
        viewing_client.project_snapshot(&mut completed_for_viewer);
        background_client.project_snapshot(&mut completed_for_background);

        assert!(viewing_client.acknowledge_surface(
            &mut completed_for_viewer,
            &surface(2),
            Some(true)
        ));

        assert_eq!(
            completed_for_viewer.agents[0].agent_status,
            AgentStatus::Idle
        );
        assert_eq!(
            completed_for_background.agents[0].agent_status,
            AgentStatus::Done
        );
    }

    #[test]
    fn stale_or_unfocused_surface_does_not_acknowledge_completion() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);

        assert!(!presentation.acknowledge_surface(&mut completed, &surface(1), Some(true)));
        assert!(!presentation.acknowledge_surface(&mut completed, &surface(2), Some(false)));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn a_finished_agent_awaits_the_user_until_its_pane_stays_on_screen_for_the_delay() {
        let delay = std::time::Duration::from_secs(1);
        let mut presentation = EndpointAgentPresentation {
            seen_delay: delay,
            ..Default::default()
        };
        let mut working = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut working);
        // The server reports it done: it doesn't count it as seen yet either.
        let mut finished = snapshot(AgentStatus::Done, 5, 2);
        presentation.project_snapshot(&mut finished);

        // Just on screen: not looked at yet.
        assert!(!presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
        assert!(presentation.awaits_user(&finished.agents[0]));

        // The window in the background starts the wait over.
        assert!(!presentation.acknowledge_surface(&mut finished, &surface(2), Some(false)));
        assert!(presentation.on_screen_since.is_empty());

        presentation.acknowledge_surface(&mut finished, &surface(2), Some(true));
        for since in presentation.on_screen_since.values_mut() {
            *since -= delay;
        }
        assert!(presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
        assert_eq!(finished.agents[0].agent_status, AgentStatus::Idle);
        assert!(!presentation.awaits_user(&finished.agents[0]));
    }

    #[test]
    fn a_finished_agent_on_screen_counts_as_seen_once_the_server_says_so() {
        let mut presentation = EndpointAgentPresentation {
            seen_delay: std::time::Duration::from_secs(1),
            ..Default::default()
        };
        let mut working = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut working);
        let mut finished = snapshot(AgentStatus::Done, 5, 2);
        presentation.project_snapshot(&mut finished);
        assert!(!presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));

        // The server's own wait ends a moment before the window's: the window follows it, so
        // leaving the tab now leaves no red count behind.
        let mut seen = snapshot(AgentStatus::Idle, 5, 3);
        presentation.project_snapshot(&mut seen);
        assert!(presentation.acknowledge_surface(&mut seen, &surface(3), Some(true)));
        assert!(!presentation.awaits_user(&seen.agents[0]));
    }

    #[test]
    fn an_agent_that_finishes_on_screen_counts_as_seen_when_the_server_counts_it() {
        let mut presentation = EndpointAgentPresentation {
            seen_delay: std::time::Duration::from_secs(1),
            ..Default::default()
        };
        let mut working = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut working);
        presentation.acknowledge_surface(&mut working, &surface(1), Some(true));

        // It finishes a moment after its tab came on screen, and the server counts it as seen.
        let mut finished = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut finished);
        assert!(presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
        let mut elsewhere = surface(2);
        elsewhere.panes.clear();
        presentation.acknowledge_surface(&mut finished, &elsewhere, Some(true));
        assert!(!presentation.awaits_user(&finished.agents[0]));
    }

    #[test]
    fn the_timer_leaves_a_snapshot_cached_while_reconnecting_alone() {
        use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
        let mut shell = ClientShellState::new(ClientShellConfig::from_config(
            &crate::config::Config::default(),
        ));
        let id = ClientEndpointId::Local;
        shell.set_endpoint_snapshot_for_generation(
            &id,
            4,
            Box::new(snapshot(AgentStatus::Working, 4, 1)),
        );
        shell.set_pane_surface(surface(1));
        let endpoint = shell
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap();
        for since in endpoint.agent_presentation.on_screen_since.values_mut() {
            *since -= std::time::Duration::from_secs(1);
        }
        // A new connection's first snapshot, same boot and revision, with a finished agent.
        shell.mark_endpoint_disconnected(&id);
        shell.cache_endpoint_snapshot_inactive_for_generation(
            &id,
            5,
            Box::new(snapshot(AgentStatus::Done, 5, 1)),
        );

        shell.tick_seen();
        assert_eq!(
            shell.snapshot.as_ref().unwrap().agents[0].state_change_seq,
            4,
            "the old connection's frame must not take in the new connection's snapshot"
        );
        let endpoint = shell
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap();
        assert_eq!(
            endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
            AgentStatus::Done
        );
    }

    #[test]
    fn the_window_in_the_background_while_reconnecting_starts_the_wait_over() {
        use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
        let mut shell = ClientShellState::new(ClientShellConfig::from_config(
            &crate::config::Config::default(),
        ));
        let id = ClientEndpointId::Local;
        shell.outer_focused = Some(true);
        shell.set_endpoint_snapshot_for_generation(
            &id,
            4,
            Box::new(snapshot(AgentStatus::Working, 4, 7)),
        );
        shell.set_pane_surface(surface(7));
        for since in shell
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap()
            .agent_presentation
            .on_screen_since
            .values_mut()
        {
            *since -= std::time::Duration::from_secs(2);
        }
        shell.mark_endpoint_disconnected(&id);
        // The new connection's first snapshot waits (the agent finished meanwhile).
        shell.cache_endpoint_snapshot_inactive_for_generation(
            &id,
            5,
            Box::new(snapshot(AgentStatus::Done, 5, 1)),
        );
        // In the background for a while (timer ticks), then back.
        shell.outer_focused = Some(false);
        shell.tick_seen();
        shell.outer_focused = Some(true);
        // The new connection's frame comes.
        shell.set_endpoint_snapshot_for_generation(
            &id,
            5,
            Box::new(snapshot(AgentStatus::Done, 5, 2)),
        );
        shell.set_pane_surface(surface(2));
        let endpoint = shell
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap();
        assert_eq!(
            endpoint.unseen_agents().count(),
            1,
            "on screen for no time since the window came back, yet counted as seen"
        );
    }

    #[test]
    fn coming_back_to_the_window_while_reconnecting_leaves_the_cached_snapshot_alone() {
        use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
        use crate::raw_input::RawInputEvent;
        let mut shell = ClientShellState::new(ClientShellConfig::from_config(
            &crate::config::Config::default(),
        ));
        let id = ClientEndpointId::Local;
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusGained]);
        shell.set_endpoint_snapshot_for_generation(
            &id,
            4,
            Box::new(snapshot(AgentStatus::Working, 4, 1)),
        );
        shell.set_pane_surface(surface(1));
        for since in shell
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap()
            .agent_presentation
            .on_screen_since
            .values_mut()
        {
            *since -= std::time::Duration::from_secs(1);
        }
        shell.mark_endpoint_disconnected(&id);
        shell.cache_endpoint_snapshot_inactive_for_generation(
            &id,
            5,
            Box::new(snapshot(AgentStatus::Done, 5, 1)),
        );
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
        shell.tick_seen();
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusGained]);
        assert_eq!(
            shell.snapshot.as_ref().unwrap().agents[0].state_change_seq,
            4,
            "the old connection's frame must not take in the new connection's snapshot"
        );
    }

    #[test]
    fn finished_agents_and_questions_await_the_user_until_their_pane_is_on_screen() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut baseline = snapshot(AgentStatus::Blocked, 4, 1);
        presentation.project_snapshot(&mut baseline);
        assert!(
            presentation.awaits_user(&baseline.agents[0]),
            "a question already open when the window opens waits for the user too"
        );
        presentation.acknowledge_surface(&mut baseline, &surface(1), Some(true));
        assert!(!presentation.awaits_user(&baseline.agents[0]));

        let mut asking = snapshot(AgentStatus::Blocked, 5, 2);
        presentation.project_snapshot(&mut asking);
        assert!(presentation.awaits_user(&asking.agents[0]));
        presentation.acknowledge_surface(&mut asking, &surface(2), Some(true));
        assert!(!presentation.awaits_user(&asking.agents[0]));

        let mut working = snapshot(AgentStatus::Working, 6, 3);
        presentation.project_snapshot(&mut working);
        assert!(!presentation.awaits_user(&working.agents[0]));
        let mut finished = snapshot(AgentStatus::Idle, 7, 4);
        presentation.project_snapshot(&mut finished);
        assert_eq!(finished.agents[0].agent_status, AgentStatus::Done);
        assert!(presentation.awaits_user(&finished.agents[0]));
        presentation.acknowledge_surface(&mut finished, &surface(4), Some(true));
        assert!(!presentation.awaits_user(&finished.agents[0]));
    }
}
