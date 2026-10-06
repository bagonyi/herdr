use super::*;
use crossterm::event::{MouseEvent, MouseEventKind};

/// A stopped session's delete button: the Nerd Font trash can, which Ghostty has built in.
const DELETE_GLYPH: &str = "\u{f1f8}";

/// A tab, sidebar row or the sidebar's resize divider under the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoveredRow {
    Tab(Rect),
    Sidebar(Rect),
    SidebarDivider(Rect),
}

impl ClientShellState {
    /// Tracks the mouse so the tab, session or space row under it is highlighted, as is the
    /// sidebar divider to show it can be dragged, and the sidebar button under it. Only motion
    /// and drags move the highlight; terminals report motion before a click or scroll at a new
    /// spot.
    pub(super) fn update_row_hover(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        if !matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) {
            return;
        }
        let before = (self.hovered_row(), self.hovered_button());
        self.row_hover = Some((mouse.column, mouse.row));
        outcome.repaint |= (self.hovered_row(), self.hovered_button()) != before;
    }

    /// The point under the mouse, unless a popup is open or something is being dragged.
    fn hover_point(&self) -> Option<(u16, u16)> {
        if self.overlay.is_some() || self.chrome_drag.is_some() {
            return None;
        }
        self.row_hover
    }

    /// Whether the mouse is over `rect`, a row whose buttons show only while it is hovered.
    pub(super) fn row_hovered(&self, rect: Rect) -> bool {
        self.hover_point()
            .is_some_and(|point| contains(rect, point))
    }

    /// The sidebar button under the mouse: a +, a session's stop button, or a stopped
    /// session's play or delete button.
    fn hovered_button(&self) -> Option<Rect> {
        let point = self.hover_point()?;
        let hits = &self.hits;
        std::iter::once(hits.new_session)
            .chain(hits.new_session_workspace.iter().map(|(rect, _)| *rect))
            .chain(hits.session_stop.iter().map(|(rect, _)| *rect))
            .chain(
                hits.stopped_sessions
                    .iter()
                    .flat_map(|hit| [hit.play, hit.delete]),
            )
            .find(|rect| contains(*rect, point))
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
            .chain(self.hits.stopped_sessions.iter().map(|hit| hit.rect))
            .chain([self.hits.stopped_header])
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
    /// divider line takes the muted overlay text colour. Then draws the sidebar buttons shown
    /// only on a hovered row and lights up the button under the mouse.
    pub(super) fn render_row_hover(&self, buffer: &mut Buffer) {
        self.render_hovered_row(buffer);
        self.render_hover_buttons(buffer);
    }

    fn render_hovered_row(&self, buffer: &mut Buffer) {
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

    /// A session's stop button and a stopped session's delete button show only while their row
    /// is hovered, and the "stopped sessions" heading then offers to hide or show its list.
    fn render_hover_buttons(&self, buffer: &mut Buffer) {
        let Some(point) = self.hover_point() else {
            return;
        };
        let palette = &self.config.palette;
        let style = Style::default().fg(palette.overlay0);
        for (stop, endpoint_id) in &self.hits.session_stop {
            let row_hovered = self
                .hits
                .machines
                .iter()
                .any(|hit| &hit.endpoint_id == endpoint_id && contains(hit.rect, point));
            if row_hovered {
                super::render::put_text(buffer, stop.x + 1, stop.y, 1, "■", style);
            }
        }
        for hit in &self.hits.stopped_sessions {
            if !hit.delete.is_empty() && self.row_hovered(hit.rect) {
                super::render::put_text(
                    buffer,
                    hit.delete.x + 1,
                    hit.delete.y,
                    1,
                    DELETE_GLYPH,
                    style,
                );
            }
        }
        let heading = self.hits.stopped_header;
        if contains(heading, point) {
            let label = if self.session_create.sidebar.folded {
                "show"
            } else {
                "hide"
            };
            super::render::put_text(
                buffer,
                heading.right().saturating_sub(5),
                heading.y,
                4,
                label,
                style,
            );
        }
        if let Some(button) = self.hovered_button() {
            if let Some(cell) = buffer.cell_mut((button.x + 1, button.y)) {
                cell.set_fg(palette.text);
            }
        }
    }
}
