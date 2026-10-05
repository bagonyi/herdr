use super::*;

/// Ours: a named session ends when its last space is closed, like a tmux session, instead of
/// getting a fresh space. The default session keeps the stock behaviour.
pub(super) struct SessionEnd {
    pub(super) named: bool,
    pub(super) had_spaces: bool,
    pub(super) ended: bool,
    /// Set when the last pane died from a signal: Herdr saved the layout before removing it so
    /// it survives a logout or shutdown that kills panes first, so the session stops but is
    /// not deleted.
    pub(super) keep_saved_state: bool,
}

impl SessionEnd {
    pub(super) fn for_active_session() -> Self {
        Self {
            named: crate::session::active_name().is_some(),
            had_spaces: false,
            ended: false,
            keep_saved_state: false,
        }
    }
}

impl HeadlessServer {
    /// Starts shutting down once a named session that had spaces has none left. Returns true
    /// from then on, so no replacement space is created. A session that starts out empty
    /// still gets its first space, and a running popup keeps it going.
    pub(super) fn end_session_after_last_space(&mut self) -> bool {
        if self.session_end.ended {
            return true;
        }
        if !self.app.state.workspaces.is_empty() {
            self.session_end.had_spaces = true;
            return false;
        }
        if !self.session_end.named
            || !self.session_end.had_spaces
            || self.handoff_in_progress
            || self.app.state.popup_pane.is_some()
        {
            return false;
        }
        self.session_end.ended = true;
        self.session_end.keep_saved_state = self.app.pane_exit_checkpoint_pending();
        info!(
            keep_saved_state = self.session_end.keep_saved_state,
            "last space closed; ending the session"
        );
        self.app.state.should_quit = true;
        true
    }

    /// Whether the session ended after its last space was closed and its saved state can go.
    pub(super) fn delete_after_shutdown(&self) -> bool {
        self.session_end.ended && !self.session_end.keep_saved_state
    }
}

/// Deletes the ended session's saved state once its server has shut down and saved.
pub(super) fn delete_ended_session() {
    let Some(name) = crate::session::active_name() else {
        return;
    };
    match crate::session::delete_session(&name) {
        Ok(_) => info!(session = %name, "deleted the ended session"),
        Err(error) => warn!(%error, session = %name, "could not delete the ended session"),
    }
}
