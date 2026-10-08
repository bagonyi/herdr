use crate::terminal::TerminalId;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, labels, and agent metadata live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last state change to Idle.
    /// False = "Done" (agent finished while user was in another workspace).
    pub seen: bool,
    /// Whether unmodified right-click gestures should be forwarded to the pane application.
    pub right_click_passthrough: bool,
    /// Fork: the green frame shown after a finished agent's pane is seen.
    pub seen_flash: Option<crate::seen_flash::SeenFlash>,
    /// Fork: when this unseen pane's tab came on screen; it counts as seen once it stays there.
    pub seen_wait_since: Option<std::time::Instant>,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
            right_click_passthrough: false,
            seen_flash: None,
            seen_wait_since: None,
        }
    }
}
