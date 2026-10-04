use super::*;
use crossterm::event::{MouseEvent, MouseEventKind};

/// A tab or sidebar row under the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoveredRow {
    Tab(Rect),
    Sidebar(Rect),
}

impl ClientShellState {
    /// Tracks the mouse so the tab, session or space row under it is highlighted. Only motion
    /// moves the highlight; terminals report motion before a click or scroll at a new spot.
    pub(super) fn update_row_hover(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        if mouse.kind != MouseEventKind::Moved {
            return;
        }
        let before = self.hovered_row();
        self.row_hover = Some((mouse.column, mouse.row));
        outcome.repaint |= self.hovered_row() != before;
    }

    pub(super) fn clear_row_hover(&mut self) -> bool {
        let highlighted = self.hovered_row().is_some();
        self.row_hover = None;
        highlighted
    }

    fn hovered_row(&self) -> Option<HoveredRow> {
        let point = self.row_hover?;
        if self.overlay.is_some() || self.chrome_drag.is_some() {
            return None;
        }
        let tabs = self
            .hits
            .tabs
            .iter()
            .map(|(rect, _)| HoveredRow::Tab(*rect));
        let sidebar = (self.hits.machines.iter().map(|hit| hit.rect))
            .chain(self.hits.workspaces.iter().map(|hit| hit.rect))
            .map(HoveredRow::Sidebar);
        tabs.chain(sidebar).find(|row| {
            let (HoveredRow::Tab(rect) | HoveredRow::Sidebar(rect)) = *row;
            contains(rect, point)
        })
    }

    /// Lightens the hovered row's plain background. The focused tab and the active, selected
    /// or dragged sidebar rows have their own backgrounds and stay as they are.
    pub(super) fn render_row_hover(&self, buffer: &mut Buffer) {
        let palette = &self.config.palette;
        let (rect, from, to) = match self.hovered_row() {
            Some(HoveredRow::Tab(rect)) => (rect, palette.surface0, palette.surface1),
            Some(HoveredRow::Sidebar(rect)) => (rect, palette.sidebar_bg, palette.selection_bg),
            None => return,
        };
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    if cell.bg == from {
                        cell.set_bg(to);
                    }
                }
            }
        }
    }
}
