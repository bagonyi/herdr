use super::*;

/// How long a session started from the sidebar may take to come online before it is opened.
/// Restoring a big session can take a while before its server accepts windows.
const SESSION_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a space asked for from another session's + waits for the switch to that session.
const WORKSPACE_CREATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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

/// A session the sidebar's right-click menu stops, kept on its confirmation popup.
#[derive(Debug)]
pub(super) struct SessionStop {
    name: String,
    delete: bool,
}

/// Work the sidebar's session buttons and menu leave for later: a space to create once its
/// session is shown, a session to open once its new server is online, and stops running in the
/// background.
#[derive(Default)]
pub(super) struct SessionCreate {
    workspace: Option<(
        ClientEndpointId,
        crate::api::schema::WorkspaceCreateParams,
        std::time::Instant,
    )>,
    session: Option<(ClientEndpointId, String, std::time::Instant)>,
    /// Windows on a named session list sessions even when theirs is the only one running.
    named_session: bool,
    stops: Vec<(String, std::thread::JoinHandle<Result<(), String>>)>,
}

impl ClientShellState {
    pub(crate) fn set_named_session(&mut self, named: bool) {
        self.session_create.named_session = named;
    }

    /// Whether the sidebar shows the sessions list rather than one session's spaces.
    pub(super) fn lists_sessions(&self) -> bool {
        self.endpoints.len() > 1 || self.session_create.named_session
    }

    /// The + after the "sessions" heading opens a popup for a new session's name; the + on a
    /// session's row creates a space in that session.
    pub(super) fn handle_session_plus_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        if contains(self.hits.new_session, point) {
            self.open_new_session_overlay("");
            outcome.repaint = true;
            return true;
        }
        let Some(endpoint_id) = self
            .hits
            .new_session_workspace
            .iter()
            .find(|(rect, _)| contains(*rect, point))
            .map(|(_, endpoint_id)| endpoint_id.clone())
        else {
            return false;
        };
        self.new_workspace_in(endpoint_id, outcome);
        true
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
        if self.activate_endpoint(endpoint_id.clone(), outcome) {
            self.session_create.workspace = Some((
                endpoint_id,
                params,
                std::time::Instant::now() + WORKSPACE_CREATE_TIMEOUT,
            ));
        }
    }

    /// Called by the runtime whenever a switch between sessions starts. A switch anywhere else,
    /// or to a chosen space, drops the space a + was waiting to create; any switch drops the
    /// wait to open a session started from the sidebar.
    pub(crate) fn endpoint_activation_requested(
        &mut self,
        endpoint_id: &ClientEndpointId,
        targeted: bool,
    ) {
        if self
            .session_create
            .workspace
            .as_ref()
            .is_some_and(|(pending, _, _)| pending != endpoint_id || targeted)
        {
            self.session_create.workspace = None;
        }
        self.session_create.session = None;
    }

    /// Called by the runtime once a switch between sessions has finished.
    pub(crate) fn take_pending_workspace_create(&mut self) -> Vec<ClientShellAction> {
        let Some((endpoint_id, params, deadline)) = self.session_create.workspace.take() else {
            return Vec::new();
        };
        if endpoint_id != self.active_endpoint_id || std::time::Instant::now() >= deadline {
            return Vec::new();
        }
        let mut outcome = ClientShellInput::default();
        self.push_endpoint_method(
            crate::api::schema::Method::WorkspaceCreate(params),
            &mut outcome,
        );
        outcome.actions
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

    /// Right-clicking another session on this computer offers to stop or delete it. The
    /// window's own session and SSH machines have no menu.
    pub(super) fn open_session_context_menu(&mut self, point: (u16, u16), x: u16, y: u16) -> bool {
        let Some((endpoint_id, name)) = self
            .hits
            .machines
            .iter()
            .find(|hit| contains(hit.rect, point))
            .and_then(|hit| {
                self.endpoints
                    .iter()
                    .find(|endpoint| endpoint.endpoint_id == hit.endpoint_id)
            })
            .filter(|endpoint| !endpoint.endpoint_id.is_local() && endpoint.is_local_session())
            .map(|endpoint| (endpoint.endpoint_id.clone(), endpoint.label.clone()))
        else {
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

    pub(super) fn confirm_session_stop(
        &mut self,
        endpoint_id: ClientEndpointId,
        name: String,
        delete: bool,
    ) {
        let snapshot = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
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
                session: Some(SessionStop { name, delete }),
            },
        ));
    }

    pub(super) fn stop_session(&mut self, stop: SessionStop, outcome: &mut ClientShellInput) {
        if self
            .session_create
            .session
            .as_ref()
            .is_some_and(|(_, name, _)| name == &stop.name)
        {
            self.session_create.session = None;
        }
        outcome.actions.push(ClientShellAction::StopSession {
            name: stop.name,
            delete: stop.delete,
        });
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
        let repaint = self
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.deadline)
            != notice;
        (actions, repaint)
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
/// the window: a stop waits for the server to exit.
pub(crate) fn spawn_session_stop(
    name: String,
    delete: bool,
) -> std::io::Result<std::thread::JoinHandle<Result<(), String>>> {
    std::thread::Builder::new()
        .name("session-stop".into())
        .spawn(move || {
            if crate::session::session_info(Some(&name)).running {
                // Another window may have stopped it in the meantime.
                if let Err(error) = crate::session::stop_session(Some(&name)) {
                    if crate::session::session_info(Some(&name)).running {
                        return Err(error);
                    }
                }
            }
            if delete {
                crate::session::delete_session(&name)?;
            }
            Ok(())
        })
}

/// Draws a sidebar + at `x` and returns its click target, one column wider on each side.
pub(super) fn render_plus(buffer: &mut Buffer, x: u16, y: u16, palette: &Palette) -> Rect {
    super::render::put_text(buffer, x, y, 1, "+", Style::default().fg(palette.overlay0));
    Rect::new(x.saturating_sub(1), y, 3, 1)
}
