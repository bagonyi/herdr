//! Fork: when a finished agent counts as seen, and a green frame around its pane meanwhile.
//!
//! Herdr marks a finished ("done") agent seen when its tab comes on screen in the focused window:
//! you switch to it, the window comes back to it, or the tab in front of it closes. This fork
//! waits until the tab has stayed on screen for `AppState::seen_delay` (`ui.seen_delay_ms`), so
//! passing a tab on the way to another leaves its agents unseen. A zero delay marks them at once.
//!
//! The pane gets a green frame as soon as its tab shows, for `SEEN_FLASH`, or until the agent
//! starts working again; leaving the tab before the wait is over takes the frame away. A pane's
//! border turns green where focus would colour it (`render_pane_borders`). A pane without a
//! border, such as a lone pane, gets the frame drawn over its outermost cells instead: the pane
//! keeps its size and the program in it doesn't redraw.

use std::time::{Duration, Instant};

use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::AppState;
use crate::detect::AgentState;
use crate::layout::{PaneId, PaneInfo};
use crate::pane::PaneState;
use crate::ui::TabSurfaceTarget;
use crate::workspace::Workspace;

/// How long the frame stays, counted from its first render.
const SEEN_FLASH: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub struct SeenFlash {
    /// When the frame was first sent to render; `None` until then.
    shown_at: Option<Instant>,
}

impl SeenFlash {
    fn until(self) -> Option<Instant> {
        self.shown_at.map(|shown_at| shown_at + SEEN_FLASH)
    }
}

impl PaneState {
    /// The pane's tab is on screen. A pane is unseen only while its agent has finished and no one
    /// has looked yet: it gets the frame when the wait starts, and is marked seen once its tab has
    /// stayed on screen for `delay`. Returns whether it was marked seen.
    pub fn see(&mut self, now: Instant, delay: Duration) -> bool {
        if self.seen {
            self.seen_wait_since = None;
            return false;
        }
        let since = *self.seen_wait_since.get_or_insert_with(|| {
            self.seen_flash = Some(SeenFlash { shown_at: None });
            now
        });
        if now.saturating_duration_since(since) < delay {
            return false;
        }
        self.seen = true;
        self.seen_wait_since = None;
        true
    }

    /// The pane's tab left the screen before its wait was over, so its agent stays unseen and
    /// the frame goes. Returns whether a frame went.
    fn stop_seen_wait(&mut self) -> bool {
        self.seen_wait_since.take().is_some() && !self.seen && self.seen_flash.take().is_some()
    }
}

/// Whether a pane's frame would sit on cells its output writes to: true for a borderless pane,
/// whose frame is drawn over its outer cells. A pane with a top or left border has its frame on
/// the border. Panes with only a right or bottom border count as borderless, to be safe.
pub(crate) fn frame_covers_output(pane: &crate::protocol::PaneSurfacePane) -> bool {
    pane.inner_rect.x == pane.rect.x && pane.inner_rect.y == pane.rect.y
}

pub(crate) fn has_seen_flash(ws: &Workspace, pane_id: PaneId) -> bool {
    ws.pane_state(pane_id)
        .is_some_and(|pane| pane.seen_flash.is_some())
}

impl AppState {
    pub(crate) fn pane_has_seen_flash(&self, ws_idx: usize, pane_id: PaneId) -> bool {
        self.workspaces
            .get(ws_idx)
            .is_some_and(|ws| has_seen_flash(ws, pane_id))
    }

    /// When the next frame ends or the next wait to count a pane as seen is over.
    pub(crate) fn next_seen_flash_deadline(&self) -> Option<Instant> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.values())
            .flat_map(|pane| {
                let wait_over = pane
                    .seen_wait_since
                    .and_then(|since| since.checked_add(self.seen_delay));
                [pane.seen_flash.and_then(SeenFlash::until), wait_over]
            })
            .flatten()
            .min()
    }

    /// Ends the waits of panes in every tab but `shown`, the one the focused window shows, if
    /// any. Returns whether a frame went, which needs a full render.
    pub(crate) fn stop_seen_waits_except(&mut self, shown: Option<TabSurfaceTarget>) -> bool {
        let mut changed = false;
        for (workspace_index, ws) in self.workspaces.iter_mut().enumerate() {
            for (tab_index, tab) in ws.tabs.iter_mut().enumerate() {
                let target = TabSurfaceTarget {
                    workspace_index,
                    tab_index,
                };
                if shown == Some(target) {
                    continue;
                }
                for pane in tab.panes.values_mut() {
                    changed |= pane.stop_seen_wait();
                }
            }
        }
        changed
    }

    /// Starts the clock on new frames and ends frames whose time is up or whose agent is working
    /// again. Returns whether a frame appeared or went away, which needs a full render.
    pub(crate) fn sync_seen_flashes(&mut self, now: Instant) -> bool {
        let terminals = &self.terminals;
        let panes = self
            .workspaces
            .iter_mut()
            .flat_map(|ws| ws.tabs.iter_mut())
            .flat_map(|tab| tab.panes.values_mut());
        let mut changed = false;
        for pane in panes {
            let Some(flash) = pane.seen_flash.as_mut() else {
                continue;
            };
            let working = terminals
                .get(&pane.attached_terminal_id)
                .is_some_and(|terminal| {
                    matches!(terminal.state, AgentState::Working | AgentState::Blocked)
                });
            if working || flash.until().is_some_and(|until| now >= until) {
                pane.seen_flash = None;
                changed = true;
            } else if flash.shown_at.is_none() {
                flash.shown_at = Some(now);
                changed = true;
            }
        }
        changed
    }
}

/// Draws the frame over the outermost cells of borderless panes that have one, after their
/// content. Bordered panes get theirs from `render_pane_borders`.
pub(crate) fn render_seen_flashes(
    app: &AppState,
    ws: &Workspace,
    pane_infos: &[PaneInfo],
    frame: &mut Frame,
) {
    let buf = frame.buffer_mut();
    let area = buf.area;
    for info in pane_infos {
        if !info.borders.is_empty() || !has_seen_flash(ws, info.id) {
            continue;
        }
        let rect = info.rect.intersection(area);
        if rect.width < 2 || rect.height < 2 {
            continue;
        }
        let right = rect.right() - 1;
        let bottom = rect.bottom() - 1;
        let edges = (rect.x..=right)
            .flat_map(|x| [(x, rect.y), (x, bottom)])
            .chain((rect.y + 1..bottom).flat_map(|y| [(rect.x, y), (right, y)]));
        for (x, y) in edges {
            let symbol = match (x == rect.x, x == right, y == rect.y, y == bottom) {
                (true, _, true, _) => "┌",
                (_, true, true, _) => "┐",
                (true, _, _, true) => "└",
                (_, true, _, true) => "┘",
                (_, _, true, _) | (_, _, _, true) => "─",
                _ => "│",
            };
            // Blank any half of a wide character the frame cuts, or the row would shift.
            if x == rect.x {
                if x > area.x && buf[(x - 1, y)].symbol().width() > 1 {
                    buf[(x - 1, y)].set_symbol(" ");
                }
                if buf[(x, y)].symbol().width() > 1 {
                    buf[(x + 1, y)].set_symbol(" ");
                }
            }
            if x == right && buf[(x - 1, y)].symbol().width() > 1 {
                buf[(x - 1, y)].set_symbol(" ");
            }
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_symbol(symbol).set_fg(app.palette.green);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry, TerminalState};
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Direction, Rect};
    use ratatui::widgets::Borders;

    fn app_with(ws: Workspace) -> AppState {
        let mut app = AppState::test_new();
        app.workspaces = vec![ws];
        app.active = Some(0);
        app
    }

    fn set_unseen(app: &mut AppState, pane_id: PaneId) {
        app.workspaces[0].pane_state_mut(pane_id).unwrap().seen = false;
    }

    /// Renders the active tab through the pane renderer and returns the buffer and pane layout.
    fn render(app: &AppState, width: u16, height: u16) -> (Buffer, Vec<PaneInfo>) {
        let runtimes = TerminalRuntimeRegistry::new();
        let surface = crate::ui::compute_tab_surface(
            app,
            &runtimes,
            Rect::new(0, 0, width, height),
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let view = crate::ui::TabSurfaceView {
            target: surface.target,
            pane_infos: &surface.pane_infos,
            split_borders: &surface.split_borders,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::render_tab_surface(app, &runtimes, view, frame))
            .unwrap();
        (terminal.backend().buffer().clone(), surface.pane_infos)
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn seeing_a_finished_agent_frames_it_once_for_two_seconds() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        set_unseen(&mut app, pane_id);

        assert!(app.mark_active_tab_seen());
        assert!(app.pane_has_seen_flash(0, pane_id));
        assert!(!app.mark_active_tab_seen());
        // The clock starts with the first render, not when the pane was seen.
        assert_eq!(app.next_seen_flash_deadline(), None);

        let now = Instant::now();
        assert!(app.sync_seen_flashes(now));
        assert_eq!(app.next_seen_flash_deadline(), Some(now + SEEN_FLASH));
        assert!(!app.sync_seen_flashes(now + SEEN_FLASH - Duration::from_millis(1)));
        assert!(app.sync_seen_flashes(now + SEEN_FLASH));
        assert!(!app.pane_has_seen_flash(0, pane_id));
        assert_eq!(app.next_seen_flash_deadline(), None);
        assert!(!app.sync_seen_flashes(now + SEEN_FLASH));
    }

    #[test]
    fn switching_to_a_tab_frames_only_its_finished_agent() {
        let mut ws = Workspace::test_new("test");
        let tab = ws.test_add_tab(None);
        let finished = ws.tabs[tab].root_pane;
        let other = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        set_unseen(&mut app, finished);

        app.switch_workspace_tab(0, tab);

        assert!(app.workspaces[0].pane_state(finished).unwrap().seen);
        assert!(app.pane_has_seen_flash(0, finished));
        assert!(!app.pane_has_seen_flash(0, other));
        // Once its frame has run, coming back to the tab doesn't frame it again.
        let now = Instant::now();
        app.sync_seen_flashes(now);
        app.sync_seen_flashes(now + SEEN_FLASH);
        app.switch_workspace_tab(0, 0);
        app.switch_workspace_tab(0, tab);
        assert!(!app.pane_has_seen_flash(0, finished));
    }

    #[test]
    fn a_finished_agent_is_framed_at_once_but_seen_only_after_the_delay() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        set_unseen(&mut app, pane_id);
        let delay = Duration::from_secs(1);
        let pane = app.workspaces[0].pane_state_mut(pane_id).unwrap();
        let now = Instant::now();

        assert!(!pane.see(now, delay));
        assert!(pane.seen_flash.is_some());
        assert!(!pane.see(now + delay - Duration::from_millis(1), delay));
        assert!(!pane.seen);
        assert!(pane.see(now + delay, delay));
        assert!(pane.seen);
        assert_eq!(pane.seen_wait_since, None);
        assert!(!pane.see(now + delay, delay));
    }

    #[test]
    fn leaving_a_tab_before_the_delay_keeps_its_agent_unseen_and_unframed() {
        let mut ws = Workspace::test_new("test");
        let tab = ws.test_add_tab(None);
        let finished = ws.tabs[tab].root_pane;
        let mut app = app_with(ws);
        app.seen_delay = Duration::from_secs(1);
        set_unseen(&mut app, finished);

        app.switch_workspace_tab(0, tab);
        assert!(!app.workspaces[0].pane_state(finished).unwrap().seen);
        assert!(app.pane_has_seen_flash(0, finished));
        // The loop wakes when the wait is over, even before the frame's clock starts.
        assert!(app.next_seen_flash_deadline().is_some());

        app.switch_workspace_tab(0, 0);
        let shown = TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        };
        assert!(app.stop_seen_waits_except(Some(shown)));
        assert!(!app.workspaces[0].pane_state(finished).unwrap().seen);
        assert!(!app.pane_has_seen_flash(0, finished));
        assert_eq!(app.next_seen_flash_deadline(), None);
        assert!(!app.stop_seen_waits_except(None));
    }

    #[test]
    fn frame_ends_when_the_agent_starts_working_again() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let terminal_id = ws.terminal_id(pane_id).unwrap().clone();
        let mut app = app_with(ws);
        let mut terminal = TerminalState::new(terminal_id.clone(), "/tmp".into());
        terminal.state = AgentState::Idle;
        app.terminals.insert(terminal_id.clone(), terminal);
        set_unseen(&mut app, pane_id);
        app.mark_active_tab_seen();
        let now = Instant::now();
        assert!(app.sync_seen_flashes(now));

        app.terminals.get_mut(&terminal_id).unwrap().state = AgentState::Working;

        assert!(app.sync_seen_flashes(now));
        assert!(!app.pane_has_seen_flash(0, pane_id));
    }

    #[tokio::test]
    async fn lone_pane_gets_a_green_frame_over_its_edges() {
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            TerminalRuntime::test_with_screen_bytes(8, 4, b"abcdefgh\r\nijklmnop"),
        );
        let mut app = app_with(ws);
        let (before, infos) = render(&app, 8, 4);
        assert_eq!(infos[0].borders, Borders::NONE);
        // The last column is the scrollbar's.
        assert_eq!(row(&before, 0), "abcdefg ");

        set_unseen(&mut app, pane_id);
        app.mark_active_tab_seen();
        let (buffer, _) = render(&app, 8, 4);

        assert_eq!(row(&buffer, 0), "┌──────┐");
        assert_eq!(row(&buffer, 1), "│jklmno│");
        assert_eq!(row(&buffer, 3), "└──────┘");
        assert_eq!(buffer[(0, 2)].fg, app.palette.green);
        assert_eq!(buffer[(3, 0)].fg, app.palette.green);
        assert_ne!(buffer[(3, 1)].fg, app.palette.green);
    }

    #[test]
    fn split_pane_border_turns_green_and_its_neighbour_keeps_its_own() {
        for gaps in [true, false] {
            let mut ws = Workspace::test_new("test");
            let left = ws.tabs[0].root_pane;
            ws.test_split(Direction::Horizontal);
            let mut app = app_with(ws);
            app.pane_gaps = gaps;
            set_unseen(&mut app, left);
            app.mark_active_tab_seen();

            let (buffer, infos) = render(&app, 20, 6);

            let rect = infos.iter().find(|info| info.id == left).unwrap().rect;
            let other = infos.iter().find(|info| info.id != left).unwrap().rect;
            // The left pane's own border, and the divider it shares without gaps.
            let divider = if gaps { rect.right() - 1 } else { rect.right() };
            for (x, y) in [(rect.x, 2), (divider, 2), (rect.x + 2, 0)] {
                assert_eq!(
                    buffer[(x, y)].fg,
                    app.palette.green,
                    "gaps={gaps} ({x},{y})"
                );
                assert_ne!(buffer[(x, y)].symbol(), " ", "gaps={gaps} ({x},{y})");
            }
            // Nothing is drawn over the pane's content, and the neighbour keeps its colours.
            assert_eq!(buffer[(divider - 1, 2)].symbol(), " ", "gaps={gaps}");
            assert_ne!(buffer[(other.right() - 1, 2)].fg, app.palette.green);
        }
    }

    #[test]
    fn wide_characters_cut_by_the_frame_are_blanked() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        set_unseen(&mut app, pane_id);
        app.mark_active_tab_seen();
        let info = PaneInfo {
            id: pane_id,
            rect: Rect::new(2, 0, 6, 3),
            inner_rect: Rect::new(2, 0, 6, 3),
            scrollbar_rect: None,
            borders: Borders::NONE,
            is_focused: true,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 3)).unwrap();
        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                // As the terminal renderer writes them: a wide character, then an empty cell.
                for (x, y) in [(1, 0), (2, 1), (6, 2)] {
                    buf[(x, y)].set_symbol("中");
                    buf[(x + 1, y)].set_symbol("");
                }
                render_seen_flashes(&app, &app.workspaces[0], &[info], frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // Left of the frame, under its left edge, and under its right edge.
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].symbol(), "┌");
        assert_eq!(buffer[(2, 1)].symbol(), "│");
        assert_eq!(buffer[(3, 1)].symbol(), " ");
        assert_eq!(buffer[(6, 2)].symbol(), "─");
        assert_eq!(buffer[(7, 2)].symbol(), "┘");
    }
}
