use super::*;
use crossterm::event::{MouseEvent, MouseEventKind};

/// A tab, sidebar row or the sidebar's resize divider under the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoveredRow {
    Tab(Rect),
    Sidebar(Rect),
    SidebarDivider(Rect),
}

impl ClientShellState {
    /// Tracks the mouse so the tab, session or space row under it is highlighted, as is the
    /// sidebar divider to show it can be dragged. Only motion and drags move the highlight;
    /// terminals report motion before a click or scroll at a new spot.
    pub(super) fn update_row_hover(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        if !matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) {
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
        if self.overlay.is_some() {
            return None;
        }
        let divider = self.hits.sidebar_divider;
        match self.chrome_drag {
            // The divider follows the mouse while dragged, so it stays lit throughout.
            Some(ClientChromeDrag::SidebarWidth) if !divider.is_empty() => {
                return Some(HoveredRow::SidebarDivider(divider));
            }
            Some(_) => return None,
            None => {}
        }
        let point = self.row_hover?;
        let tabs = self
            .hits
            .tabs
            .iter()
            .map(|(rect, _)| HoveredRow::Tab(*rect));
        let sidebar = (self.hits.machines.iter().map(|hit| hit.rect))
            .chain(self.hits.workspaces.iter().map(|hit| hit.rect))
            .map(HoveredRow::Sidebar);
        tabs.chain(sidebar)
            .chain([HoveredRow::SidebarDivider(divider)])
            .find(|row| {
                let (HoveredRow::Tab(rect)
                | HoveredRow::Sidebar(rect)
                | HoveredRow::SidebarDivider(rect)) = *row;
                contains(rect, point)
            })
    }

    /// Lightens the hovered row's plain background. The focused tab and the active, selected
    /// or dragged sidebar rows have their own backgrounds and stay as they are. A hovered
    /// divider line takes the muted overlay text colour.
    pub(super) fn render_row_hover(&self, buffer: &mut Buffer) {
        let palette = &self.config.palette;
        let (rect, recolor): (Rect, &dyn Fn(&mut ratatui::buffer::Cell)) = match self.hovered_row()
        {
            Some(HoveredRow::Tab(rect)) => (rect, &|cell| {
                if cell.bg == palette.surface0 {
                    cell.set_bg(palette.surface1);
                }
            }),
            Some(HoveredRow::Sidebar(rect)) => (rect, &|cell| {
                if cell.bg == palette.sidebar_bg {
                    cell.set_bg(palette.selection_bg);
                }
            }),
            Some(HoveredRow::SidebarDivider(rect)) => (rect, &|cell| {
                if cell.fg == palette.surface_dim {
                    cell.set_fg(palette.overlay0);
                }
            }),
            None => return,
        };
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    recolor(cell);
                }
            }
        }
    }
}
