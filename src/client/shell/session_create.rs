use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::*;

/// How long a session started from the sidebar may take to come online before it is opened.
/// Restoring a big session can take a while before its server accepts windows.
const SESSION_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long work waiting for a switch to another session, such as a space asked for from that
/// session's +, waits for the switch to finish.
const SWITCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a popup opened from one of the sidebar's + buttons creates.
#[derive(Debug)]
pub(super) enum SessionPrompt {
    NewSession,
    NewWorkspace {
        endpoint_id: ClientEndpointId,
        source_workspace_id: Option<String>,
        cwd: Option<String>,
        suggested_name: String,
    },
}

/// A session the sidebar's stop button or right-click menu stops or deletes, kept on its
/// confirmation popup. A stopped session has no endpoint.
#[derive(Debug)]
pub(super) struct SessionStop {
    endpoint_id: Option<ClientEndpointId>,
    name: String,
    delete: bool,
}

/// What the sessions sidebar shows below the running sessions.
#[derive(Default)]
pub(super) struct SessionsSidebar {
    /// Saved sessions on this computer that aren't running, A-Z.
    pub(super) stopped: Vec<String>,
    /// Stopped sessions started from the sidebar that aren't up yet, with when to give up.
    pub(super) starting: HashMap<String, std::time::Instant>,
    /// Whether the stopped sessions are folded away under their heading.
    pub(super) folded: bool,
    /// Whether this window's own session isn't running: stopped, so it shows among the stopped
    /// ones, or deleted. Its row is left out of the running ones meanwhile.
    pub(super) own_hidden: bool,
    /// Whether this window was opened on a named session, which can be stopped from here.
    pub(super) own_named: bool,
}

pub(super) struct StoppedSessionHit {
    pub(super) rect: Rect,
    pub(super) play: Rect,
    pub(super) delete: Rect,
    pub(super) name: String,
}

/// Work waiting for a switch to another session to finish.
enum AfterSwitch {
    CreateWorkspace(crate::api::schema::WorkspaceCreateParams),
    OpenWorkspaceMenu {
        workspace_id: String,
        x: u16,
        y: u16,
    },
}

/// Work the sidebar's session buttons and menus leave for later: work waiting for a switch to
/// another session, a space menu to open once the switch has been drawn, a session to open once
/// its new server is online, a stop waiting for the window to show another session, and stops
/// running in the background.
#[derive(Default)]
pub(super) struct SessionCreate {
    after_switch: Option<(ClientEndpointId, AfterSwitch, std::time::Instant)>,
    /// A space's menu, opened beside its row once the session switched to is on screen.
    workspace_menu: Option<(String, u16, u16)>,
    session: Option<(ClientEndpointId, String, std::time::Instant)>,
    /// Stops the session that was on screen once the window shows another; kept apart from
    /// `after_switch`, so nothing else waiting for a switch can replace it.
    pending_stop: Option<(SessionStop, std::time::Instant)>,
    /// Windows on a named session list sessions even when theirs is the only one running.
    named_session: bool,
    stops: Vec<(String, std::thread::JoinHandle<Result<(), String>>)>,
    pub(super) sidebar: SessionsSidebar,
    /// Set while a ctrl+click, taken as a right-click, waits for its release.
    ctrl_click: bool,
}

impl ClientShellState {
    pub(crate) fn set_named_session(&mut self, named: bool) {
        self.session_create.named_session = named;
        self.session_create.sidebar.own_named = named;
    }

    /// Whether the sidebar shows the sessions list rather than one session's spaces.
    pub(super) fn lists_sessions(&self) -> bool {
        self.endpoints.len() > 1
            || self.session_create.named_session
            || !self.session_create.sidebar.stopped.is_empty()
    }

    /// Called by the runtime with what the sidebar shows about saved sessions. Returns whether
    /// the sidebar changed, and whether this window's own session is running again, so the
    /// runtime can reconnect to it at once.
    pub(crate) fn set_saved_sessions(
        &mut self,
        saved: crate::client::saved_sessions::SavedSessions,
    ) -> (bool, bool) {
        let own_hidden = !saved.own_running;
        let sidebar = &mut self.session_create.sidebar;
        let own_back = sidebar.own_hidden && !own_hidden;
        sidebar
            .starting
            .retain(|name, _| saved.stopped.contains(name));
        if sidebar.stopped == saved.stopped && sidebar.own_hidden == own_hidden {
            return (false, false);
        }
        sidebar.stopped = saved.stopped;
        sidebar.own_hidden = own_hidden;
        (true, own_back)
    }

    /// The name of a session on this computer shown as `endpoint_id`.
    fn session_name(&self, endpoint_id: &ClientEndpointId) -> Option<String> {
        if endpoint_id.is_local() {
            return crate::session::active_name();
        }
        self.endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .filter(|endpoint| endpoint.is_local_session())
            .map(|endpoint| endpoint.label.clone())
    }

    /// The sessions sidebar's buttons: the + after the "sessions" heading opens a popup for a
    /// new session's name, a session's + creates a space in it and its stop button stops it.
    /// Clicking the "stopped sessions" heading folds the list, and a stopped session's play button
    /// starts it; clicking its name does nothing.
    pub(super) fn handle_session_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        if contains(self.hits.new_session, point) {
            self.open_new_session_overlay("");
            outcome.repaint = true;
            return true;
        }
        let hit = |targets: &[(Rect, ClientEndpointId)]| {
            targets
                .iter()
                .find(|(rect, _)| contains(*rect, point))
                .map(|(_, endpoint_id)| endpoint_id.clone())
        };
        if let Some(endpoint_id) = hit(&self.hits.new_session_workspace) {
            self.new_workspace_in(endpoint_id, outcome);
            return true;
        }
        // A stop button shows, and works, only while its row is hovered.
        let stop = hit(&self.hits.session_stop).filter(|endpoint_id| {
            self.hits
                .machines
                .iter()
                .any(|hit| &hit.endpoint_id == endpoint_id && self.row_hovered(hit.rect))
        });
        if let Some(endpoint_id) = stop {
            if let Some(name) = self.session_name(&endpoint_id) {
                self.confirm_session_stop(Some(endpoint_id), name, false);
                outcome.repaint = true;
            }
            return true;
        }
        if contains(self.hits.stopped_header, point) {
            self.session_create.sidebar.folded = !self.session_create.sidebar.folded;
            outcome.repaint = true;
            return true;
        }
        let Some(hit) = self
            .hits
            .stopped_sessions
            .iter()
            .find(|hit| contains(hit.rect, point))
        else {
            return false;
        };
        let name = hit.name.clone();
        if contains(hit.delete, point) && self.row_hovered(hit.rect) {
            self.confirm_session_stop(None, name, true);
            outcome.repaint = true;
            return true;
        }
        if contains(hit.play, point) {
            self.start_stopped_session(&name, outcome);
        }
        true
    }

    /// Starts a stopped session in the background.
    pub(super) fn start_stopped_session(&mut self, name: &str, outcome: &mut ClientShellInput) {
        outcome.repaint = true;
        if self.session_create.sidebar.starting.contains_key(name) {
            return;
        }
        self.session_create.sidebar.starting.insert(
            name.to_owned(),
            std::time::Instant::now() + SESSION_START_TIMEOUT,
        );
        outcome.actions.push(ClientShellAction::StartSession {
            name: name.to_owned(),
        });
    }

    fn open_new_session_overlay(&mut self, name: &str) {
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "new session",
            input: TextEditor::new(name, false),
            target: ClientRenameTarget::Session(SessionPrompt::NewSession),
        }));
    }

    fn new_workspace_in(&mut self, endpoint_id: ClientEndpointId, outcome: &mut ClientShellInput) {
        if endpoint_id == self.active_endpoint_id {
            self.record_binding(
                crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
                outcome,
            );
            return;
        }
        let focused = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
            .and_then(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.focused)
            })
            .map(|workspace| {
                (
                    workspace.workspace_id.clone(),
                    workspace.new_workspace_cwd.clone(),
                )
            });
        let (source_workspace_id, cwd) = focused.unzip();
        if self.config.prompt_new_workspace_name {
            let suggested_name = cwd
                .as_deref()
                .map(std::path::Path::new)
                .map(crate::workspace::derive_label_from_cwd)
                .unwrap_or_else(|| "workspace".to_owned());
            self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                title: "new workspace",
                input: TextEditor::new(&suggested_name, true),
                target: ClientRenameTarget::Session(SessionPrompt::NewWorkspace {
                    endpoint_id,
                    source_workspace_id,
                    cwd,
                    suggested_name,
                }),
            }));
            outcome.repaint = true;
        } else {
            self.create_workspace_in(
                endpoint_id,
                crate::api::schema::WorkspaceCreateParams {
                    source_workspace_id,
                    cwd: None,
                    focus: true,
                    label: None,
                    env: Default::default(),
                },
                outcome,
            );
        }
    }

    pub(super) fn save_session_prompt(
        &mut self,
        prompt: SessionPrompt,
        name: &str,
        outcome: &mut ClientShellInput,
    ) {
        match prompt {
            SessionPrompt::NewSession => self.start_session(name, outcome),
            SessionPrompt::NewWorkspace {
                endpoint_id,
                source_workspace_id,
                cwd,
                suggested_name,
            } => self.create_workspace_in(
                endpoint_id,
                crate::api::schema::WorkspaceCreateParams {
                    source_workspace_id,
                    cwd,
                    focus: true,
                    label: (!name.is_empty() && name != suggested_name).then(|| name.to_owned()),
                    env: Default::default(),
                },
                outcome,
            ),
        }
    }

    /// Requests go to the session on screen, so another session is shown first and the space
    /// is created once that switch has finished.
    fn create_workspace_in(
        &mut self,
        endpoint_id: ClientEndpointId,
        params: crate::api::schema::WorkspaceCreateParams,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        if endpoint_id == self.active_endpoint_id {
            self.push_endpoint_method(crate::api::schema::Method::WorkspaceCreate(params), outcome);
            return;
        }
        self.hold_recent_focus(endpoint_id.clone());
        self.after_switch_to(endpoint_id, AfterSwitch::CreateWorkspace(params), outcome);
    }

    fn after_switch_to(
        &mut self,
        endpoint_id: ClientEndpointId,
        work: AfterSwitch,
        outcome: &mut ClientShellInput,
    ) {
        if self.activate_endpoint(endpoint_id.clone(), outcome) {
            self.session_create.after_switch = Some((
                endpoint_id,
                work,
                std::time::Instant::now() + SWITCH_TIMEOUT,
            ));
        }
    }

    /// Right-clicking a space of another session switches to that session, then opens the
    /// space's menu: space actions only go to the session on screen.
    pub(super) fn open_other_session_workspace_menu(
        &mut self,
        point: (u16, u16),
        x: u16,
        y: u16,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(hit) =
            self.hits.workspaces.iter().find(|hit| {
                hit.endpoint_id != self.active_endpoint_id && contains(hit.rect, point)
            })
        else {
            return false;
        };
        let endpoint_id = hit.endpoint_id.clone();
        let workspace_id = hit.workspace_id.clone();
        self.after_switch_to(
            endpoint_id,
            AfterSwitch::OpenWorkspaceMenu { workspace_id, x, y },
            outcome,
        );
        true
    }

    /// Called by the runtime whenever a switch between sessions starts. A switch anywhere else,
    /// or to a chosen space, drops the work waiting for the switch, but not a pending stop,
    /// which only needs the window to show another session; any switch drops the wait to open a
    /// session started from the sidebar.
    pub(crate) fn endpoint_activation_requested(
        &mut self,
        endpoint_id: &ClientEndpointId,
        targeted: bool,
    ) {
        if self
            .session_create
            .after_switch
            .as_ref()
            .is_some_and(|(pending, _, _)| pending != endpoint_id || targeted)
        {
            self.session_create.after_switch = None;
        }
        self.session_create.workspace_menu = None;
        self.session_create.session = None;
    }

    /// Called by the runtime once a switch between sessions has finished.
    pub(crate) fn take_after_switch(&mut self) -> Vec<ClientShellAction> {
        let mut outcome = ClientShellInput::default();
        let now = std::time::Instant::now();
        if let Some((stop, deadline)) = self.session_create.pending_stop.take() {
            if stop.endpoint_id.as_ref() == Some(&self.active_endpoint_id) {
                // Still on the session to stop: keep waiting, until `poll_sessions` gives up.
                self.session_create.pending_stop = Some((stop, deadline));
            } else if now < deadline {
                outcome.actions.push(ClientShellAction::StopSession {
                    name: stop.name,
                    delete: stop.delete,
                });
            }
        }
        let Some((endpoint_id, work, deadline)) = self.session_create.after_switch.take() else {
            return outcome.actions;
        };
        match work {
            _ if endpoint_id != self.active_endpoint_id || now >= deadline => {}
            AfterSwitch::CreateWorkspace(params) => self.push_endpoint_method(
                crate::api::schema::Method::WorkspaceCreate(params),
                &mut outcome,
            ),
            // Opened by `poll_sessions` once the switch is on screen, beside the space's row.
            AfterSwitch::OpenWorkspaceMenu { workspace_id, x, y } => {
                self.session_create.workspace_menu = Some((workspace_id, x, y));
            }
        }
        outcome.actions
    }

    /// Opens a space's menu asked for from another session, once that session is on screen: by
    /// the space's row if it shows, else where it was asked for. A popup opened meanwhile wins.
    fn open_waiting_workspace_menu(&mut self) -> bool {
        let Some((workspace_id, x, y)) = self.session_create.workspace_menu.take() else {
            return false;
        };
        if self.overlay.is_some() {
            return false;
        }
        let y = self
            .hits
            .workspaces
            .iter()
            .find(|hit| {
                hit.endpoint_id == self.active_endpoint_id && hit.workspace_id == workspace_id
            })
            .map_or(y, |hit| hit.rect.y);
        self.open_workspace_context_menu(workspace_id, x, y);
        self.overlay.is_some()
    }

    /// Starts a server for a new named session, like `herdr --session <name>`, and opens it
    /// once it is online. A session that is already listed is opened straight away. A name
    /// that can't be used keeps the popup open so it can be corrected.
    fn start_session(&mut self, typed: &str, outcome: &mut ClientShellInput) {
        outcome.repaint = true;
        if typed.is_empty() {
            return;
        }
        let name = session_name(typed);
        let error = if name == crate::session::DEFAULT_SESSION_NAME {
            Some("The default session isn't listed in the sidebar; pick another name.".to_owned())
        } else {
            crate::session::validate_name(&name).err()
        };
        if let Some(error) = error {
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Rejected,
                "new_session",
                "New session",
                error,
            );
            self.open_new_session_overlay(typed);
            return;
        }
        // Session folders usually ignore case, so "work" would reopen a session saved as "Work":
        // use the existing spelling.
        let name = crate::session::list_sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|session| session.name)
            .find(|existing| existing.eq_ignore_ascii_case(&name))
            .unwrap_or(name);
        let name = name.as_str();
        let endpoint_id = if crate::session::active_name().as_deref() == Some(name) {
            ClientEndpointId::Local
        } else {
            ClientEndpointId::Ssh(crate::client::endpoint::local_session_profile_id(name))
        };
        if self
            .endpoints
            .iter()
            .any(|endpoint| endpoint.endpoint_id == endpoint_id)
        {
            self.activate_endpoint(endpoint_id, outcome);
            return;
        }
        self.session_create.session = Some((
            endpoint_id,
            name.to_owned(),
            std::time::Instant::now() + SESSION_START_TIMEOUT,
        ));
        outcome.actions.push(ClientShellAction::StartSession {
            name: name.to_owned(),
        });
    }

    /// Right-clicking a running session on this computer offers to stop or delete it, and a
    /// stopped one to start or delete it. SSH machines and the default session have no menu.
    pub(super) fn open_session_context_menu(&mut self, point: (u16, u16), x: u16, y: u16) -> bool {
        let running = self
            .hits
            .machines
            .iter()
            .find(|hit| contains(hit.rect, point))
            .and_then(|hit| {
                let name = self.session_name(&hit.endpoint_id)?;
                Some((Some(hit.endpoint_id.clone()), name))
            });
        let stopped = || {
            self.hits
                .stopped_sessions
                .iter()
                .find(|hit| contains(hit.rect, point))
                .map(|hit| (None, hit.name.clone()))
        };
        let Some((endpoint_id, name)) = running.or_else(stopped) else {
            return false;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Session { endpoint_id, name },
            x,
            y,
            highlighted: 0,
        }));
        true
    }

    /// On macOS a ctrl+click is a right-click. Outside panes, where programs may use ctrl+click
    /// themselves, Herdr takes it as one; not in the narrow-screen layout, which works with
    /// plain clicks.
    pub(super) fn ctrl_click_as_right_click(&mut self, mouse: MouseEvent) -> MouseEvent {
        let point = (mouse.column, mouse.row);
        let as_right = |kind| MouseEvent {
            kind,
            modifiers: mouse.modifiers.difference(KeyModifiers::CONTROL),
            ..mouse
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if mouse.modifiers.contains(KeyModifiers::CONTROL)
                    && self.overlay.is_none()
                    && !self.mobile_layout_active()
                    && !self.hits.panes.iter().any(|hit| contains(hit.rect, point))
                    && !self
                        .hits
                        .popup
                        .as_ref()
                        .is_some_and(|hit| contains(hit.rect, point)) =>
            {
                self.session_create.ctrl_click = true;
                as_right(MouseEventKind::Down(MouseButton::Right))
            }
            MouseEventKind::Drag(MouseButton::Left) if self.session_create.ctrl_click => {
                as_right(MouseEventKind::Drag(MouseButton::Right))
            }
            MouseEventKind::Up(MouseButton::Left)
                if std::mem::take(&mut self.session_create.ctrl_click) =>
            {
                as_right(MouseEventKind::Up(MouseButton::Right))
            }
            // Any other press ends a ctrl+click whose release never came.
            MouseEventKind::Down(_) => {
                self.session_create.ctrl_click = false;
                mouse
            }
            _ => mouse,
        }
    }

    pub(super) fn confirm_session_stop(
        &mut self,
        endpoint_id: Option<ClientEndpointId>,
        name: String,
        delete: bool,
    ) {
        let snapshot = self
            .endpoints
            .iter()
            .find(|endpoint| Some(&endpoint.endpoint_id) == endpoint_id.as_ref())
            .and_then(|endpoint| endpoint.snapshot.as_deref());
        let count = |count: usize, noun: &str| match count {
            1 => format!("1 {noun}"),
            count => format!("{count} {noun}s"),
        };
        let mut detail = name.clone();
        if let Some(snapshot) = snapshot {
            detail.push_str(" — ");
            detail.push_str(&count(snapshot.workspaces.len(), "workspace"));
            let agents = snapshot.agents.len();
            if agents > 0 {
                let working = snapshot
                    .agents
                    .iter()
                    .filter(|agent| agent.agent_status == crate::api::schema::AgentStatus::Working)
                    .count();
                detail.push_str(&format!(", {} ({working} working)", count(agents, "agent")));
            }
        }
        self.overlay = Some(ClientShellOverlay::ConfirmClose(
            ClientConfirmCloseOverlay {
                workspace_id: String::new(),
                tab_target: None,
                title: if delete {
                    "Delete session? Its workspaces are forgotten.".to_owned()
                } else {
                    "Stop session? Starting it again restores it.".to_owned()
                },
                detail,
                session: Some(SessionStop {
                    endpoint_id,
                    name,
                    delete,
                }),
            },
        ));
    }

    /// Stopping the session on screen switches to another running session first, so the window
    /// stays open; it closes only with the last running session.
    pub(super) fn stop_session(&mut self, stop: SessionStop, outcome: &mut ClientShellInput) {
        if self
            .session_create
            .session
            .as_ref()
            .is_some_and(|(_, name, _)| name == &stop.name)
        {
            self.session_create.session = None;
        }
        if stop.endpoint_id.as_ref() == Some(&self.active_endpoint_id) {
            if let Some(next) = self.neighbouring_session(&self.active_endpoint_id) {
                if self.activate_endpoint(next, outcome) {
                    self.session_create.pending_stop =
                        Some((stop, std::time::Instant::now() + SWITCH_TIMEOUT));
                }
                return;
            }
        }
        outcome.actions.push(ClientShellAction::StopSession {
            name: stop.name,
            delete: stop.delete,
        });
    }

    /// The running session listed after `endpoint_id`, or else before it, that can be shown.
    fn neighbouring_session(&self, endpoint_id: &ClientEndpointId) -> Option<ClientEndpointId> {
        let sessions = self
            .endpoints
            .iter()
            .filter(|endpoint| {
                &endpoint.endpoint_id == endpoint_id
                    || (endpoint.is_local_session()
                        && self.endpoint_projection_available(&endpoint.endpoint_id))
            })
            .map(|endpoint| &endpoint.endpoint_id)
            .collect::<Vec<_>>();
        let index = sessions.iter().position(|id| *id == endpoint_id)?;
        sessions
            .get(index + 1)
            .or_else(|| sessions.get(index.checked_sub(1)?))
            .map(|id| (*id).clone())
    }

    pub(crate) fn track_session_stop(
        &mut self,
        name: String,
        stop: std::io::Result<std::thread::JoinHandle<Result<(), String>>>,
    ) {
        match stop {
            Ok(handle) => self.session_create.stops.push((name, handle)),
            Err(error) => self.session_stop_failed(&name, error.to_string()),
        }
    }

    fn session_stop_failed(&mut self, name: &str, error: String) {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Rejected,
            "stop_session",
            "Stop session",
            format!("Session {name} could not be stopped: {error}"),
        );
    }

    /// Called by the runtime between events: opens a session started from the sidebar once it
    /// is online and reports stops that failed. Returns the actions to run and whether to
    /// repaint.
    pub(crate) fn poll_sessions(
        &mut self,
        now: std::time::Instant,
    ) -> (Vec<ClientShellAction>, bool) {
        let mut actions = Vec::new();
        let notice = self
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.deadline);
        if let Some(endpoint_id) = self.take_started_session(now) {
            actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: None,
            });
        }
        let menu = self.open_waiting_workspace_menu();
        // A started session whose server never came up gets its play button back.
        let failed = self
            .session_create
            .sidebar
            .starting
            .iter()
            .filter(|(_, deadline)| now >= **deadline)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in failed {
            self.session_create.sidebar.starting.remove(&name);
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Rejected,
                "new_session",
                "New session",
                format!("Session {name} did not come online."),
            );
        }
        if let Some((stop, _)) = self
            .session_create
            .pending_stop
            .take_if(|(_, deadline)| now >= *deadline)
        {
            self.session_stop_failed(
                &stop.name,
                "the window could not switch to another session first".to_owned(),
            );
        }
        if self
            .session_create
            .stops
            .iter()
            .any(|(_, handle)| handle.is_finished())
        {
            for (name, handle) in std::mem::take(&mut self.session_create.stops) {
                if !handle.is_finished() {
                    self.session_create.stops.push((name, handle));
                    continue;
                }
                let result = handle
                    .join()
                    .unwrap_or_else(|_| Err("the stop was interrupted".to_owned()));
                if let Err(error) = result {
                    self.session_stop_failed(&name, error);
                }
            }
        }
        let repaint = menu
            || self
                .visible_endpoint_notice
                .as_ref()
                .map(|notice| notice.deadline)
                != notice;
        (actions, repaint)
    }

    /// Called by the runtime as the window closes with its own session: lets stops and deletes
    /// it started finish, so a deleted session doesn't stay behind, waiting at most `timeout`.
    pub(crate) fn finish_session_stops(&mut self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while self
            .session_create
            .stops
            .iter()
            .any(|(_, handle)| !handle.is_finished())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// The session started from the sidebar, once it is online and can be shown.
    pub(super) fn take_started_session(
        &mut self,
        now: std::time::Instant,
    ) -> Option<ClientEndpointId> {
        let (endpoint_id, name, deadline) = self.session_create.session.as_ref()?;
        let available = self.endpoint_projection_available(endpoint_id);
        // Switching closes any popup and ends copy mode, so wait until those are done.
        let busy = self.overlay.is_some() || self.mode == ClientShellMode::Copy;
        if available && !busy {
            let endpoint_id = endpoint_id.clone();
            self.session_create.session = None;
            return Some(endpoint_id);
        }
        if now >= *deadline {
            let body = format!("Session {name} did not come online.");
            self.session_create.session = None;
            if !available {
                self.push_endpoint_notice(
                    ClientEndpointNoticeKind::Rejected,
                    "new_session",
                    "New session",
                    body,
                );
            }
        }
        None
    }

    pub(crate) fn session_start_failed(&mut self, name: &str, error: &std::io::Error) {
        self.session_create.session = None;
        self.session_create.sidebar.starting.remove(name);
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Rejected,
            "new_session",
            "New session",
            format!("Session {name} could not be started: {error}"),
        );
    }
}

/// Session names may only use ASCII letters, numbers, '.', '_' and '-'. Spaces become '-' and
/// anything else is dropped, so "Test session" becomes "Test-session".
fn session_name(typed: &str) -> String {
    typed
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Spawns a detached server for the named session, as `herdr --session <name>` does when the
/// session isn't running. Its first space opens in the home directory. The window outlives
/// the servers it starts, so on Unix the child is returned to be reaped once it exits.
pub(crate) fn spawn_session_server(name: &str) -> std::io::Result<Option<std::process::Child>> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["--session", name, "server"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env_remove(crate::api::SOCKET_PATH_ENV_VAR)
        .env_remove("HERDR_CLIENT_SOCKET_PATH");
    let cwd = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::current_dir().ok().map(Into::into));
    if let Some(cwd) = cwd {
        command.env(crate::server::autodetect::STARTUP_CWD_ENV_VAR, cwd);
    }
    crate::platform::detach_server_daemon_command(&mut command);
    #[cfg(unix)]
    {
        command.spawn().map(Some)
    }
    #[cfg(not(unix))]
    {
        crate::platform::launch_server_daemon_command(&mut command).map(|_| None)
    }
}

/// Stops a session on this computer, and deletes it afterwards if asked, without holding up
/// the window: a stop waits for the server to exit. A session stopped on its own stays stopped
/// when plain `herdr` starts the others.
pub(crate) fn spawn_session_stop(
    name: String,
    delete: bool,
) -> std::io::Result<std::thread::JoinHandle<Result<(), String>>> {
    std::thread::Builder::new()
        .name("session-stop".into())
        .spawn(move || {
            if crate::session::session_info(Some(&name)).running {
                let stopped = if delete {
                    crate::session::stop_session(Some(&name))
                } else {
                    crate::client::saved_sessions::stop_keeping_stopped(&name)
                };
                // Another window may have stopped it in the meantime.
                if let Err(error) = stopped {
                    if crate::session::session_info(Some(&name)).running {
                        return Err(error);
                    }
                }
            } else if !delete {
                crate::client::saved_sessions::keep_stopped(&name);
            }
            if delete {
                crate::session::delete_session(&name)?;
            }
            Ok(())
        })
}

/// The "stopped sessions" heading. Folded, the count shows on the right, ending in the +
/// column; hovering the heading shows "hide" or "show" there instead, as a macOS sidebar section
/// does (see `render_row_hover`).
pub(super) fn render_stopped_heading(
    buffer: &mut Buffer,
    rect: Rect,
    sidebar: &SessionsSidebar,
    palette: &Palette,
    hits: &mut ShellHitMap,
) {
    let style = Style::default().fg(palette.overlay0);
    super::render::put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width.saturating_sub(6),
        " stopped sessions",
        style.add_modifier(Modifier::BOLD),
    );
    if sidebar.folded {
        let count = sidebar.stopped.len().to_string();
        let width = super::render::display_width(&count);
        super::render::put_text(
            buffer,
            rect.right().saturating_sub(width.saturating_add(1)),
            rect.y,
            width,
            &count,
            style,
        );
    }
    hits.stopped_header = rect;
}

/// A stopped session's row: its name, a play button in the + column ("…" while it starts) and a
/// delete button that shows while the row is hovered (see `render_row_hover`).
pub(super) fn render_stopped_session(
    buffer: &mut Buffer,
    rect: Rect,
    sidebar: &SessionsSidebar,
    index: usize,
    mouse_capture: bool,
    palette: &Palette,
    hits: &mut ShellHitMap,
) {
    let Some(name) = sidebar.stopped.get(index) else {
        return;
    };
    let style = Style::default().fg(palette.overlay0);
    super::render::put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width.saturating_sub(6),
        &format!("   {name}"),
        style,
    );
    let x = rect.right().saturating_sub(2);
    let starting = sidebar.starting.contains_key(name);
    // The delete icon is drawn two cells wide, so it starts one column left of a running
    // session's stop button, leaving the same one-cell gap before the play button as between a
    // stop button and its +. A session that is starting can't be deleted.
    let delete = if mouse_capture && !starting && x > rect.x.saturating_add(7) {
        button_rect(x.saturating_sub(3), rect.y)
    } else {
        Rect::default()
    };
    let play = if starting {
        super::render::put_text(buffer, x, rect.y, 1, "…", style);
        Rect::default()
    } else if mouse_capture && x > rect.x.saturating_add(4) {
        render_button(buffer, x, rect.y, "▶", palette)
    } else {
        Rect::default()
    };
    hits.stopped_sessions.push(StoppedSessionHit {
        rect,
        play,
        delete,
        name: name.clone(),
    });
}

/// Draws a sidebar + at `x` and returns its click target, one column wider on each side.
pub(super) fn render_plus(buffer: &mut Buffer, x: u16, y: u16, palette: &Palette) -> Rect {
    render_button(buffer, x, y, "+", palette)
}

/// Draws a one-column sidebar button, such as a stopped session's play button, and returns its
/// click target, one column wider on each side.
pub(super) fn render_button(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    glyph: &str,
    palette: &Palette,
) -> Rect {
    super::render::put_text(
        buffer,
        x,
        y,
        1,
        glyph,
        Style::default().fg(palette.overlay0),
    );
    button_rect(x, y)
}

/// The click target of a sidebar button drawn at `x`, one column wider on each side. Buttons
/// shown only while their row is hovered are drawn by `render_row_hover`.
pub(super) fn button_rect(x: u16, y: u16) -> Rect {
    Rect::new(x.saturating_sub(1), y, 3, 1)
}
