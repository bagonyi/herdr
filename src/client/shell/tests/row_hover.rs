use super::*;

fn moved(column: u16, row: u16) -> RawInputEvent {
    RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn hovering_a_tab_or_space_row_highlights_it() {
    let mut snapshot = snapshot();
    let mut other_workspace = snapshot.workspaces[0].clone();
    other_workspace.workspace_id = "ws_2".into();
    other_workspace.active_tab_id = "tab_3".into();
    other_workspace.number = 2;
    other_workspace.label = "other".into();
    other_workspace.focused = false;
    snapshot.workspaces.push(other_workspace);
    for (tab_id, workspace_id, number) in [("tab_2", "ws_1", 2), ("tab_3", "ws_2", 1)] {
        snapshot.tabs.push(ClientShellTab {
            tab_id: tab_id.into(),
            workspace_id: workspace_id.into(),
            number,
            label: "agent".into(),
            custom_label: true,
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Idle,
        });
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    let plain = state.compose(140, 20).expect("frame");
    let tab = |state: &ClientShellState, tab_id: &str| {
        state
            .hits
            .tabs
            .iter()
            .find(|(_, id)| id == tab_id)
            .map(|(rect, _)| *rect)
            .expect("visible tab")
    };
    let space = |state: &ClientShellState, workspace_id: &str| {
        state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == workspace_id)
            .map(|hit| hit.rect)
            .expect("visible space")
    };
    let backgrounds = |frame: &FrameData, rect: Rect| {
        (rect.x..rect.right())
            .map(|x| frame.cells[rect.y as usize * frame.width as usize + x as usize].bg)
            .collect::<Vec<_>>()
    };
    let palette = state.config.palette.clone();
    let color = crate::protocol::color_to_u32;

    let other_tab = tab(&state, "tab_2");
    let outcome = state.handle_raw_events(vec![moved(other_tab.x + 1, other_tab.y)]);
    assert!(outcome.repaint, "entering a tab repaints");
    let outcome = state.handle_raw_events(vec![moved(other_tab.x + 2, other_tab.y)]);
    assert!(!outcome.repaint, "moving within the same tab does not");
    let frame = state.compose(140, 20).expect("frame");
    assert!(backgrounds(&frame, other_tab)
        .iter()
        .all(|bg| *bg == color(palette.surface1)));

    let other_space = space(&state, "ws_2");
    state.handle_raw_events(vec![moved(other_space.x + 1, other_space.y)]);
    let frame = state.compose(140, 20).expect("frame");
    assert!(backgrounds(&frame, other_space)
        .iter()
        .any(|bg| *bg == color(palette.selection_bg)));
    assert_eq!(
        backgrounds(&frame, other_tab),
        backgrounds(&plain, other_tab),
        "only the row under the mouse is highlighted"
    );

    for rect in [tab(&state, "tab_1"), space(&state, "ws_1")] {
        state.handle_raw_events(vec![moved(rect.x + 1, rect.y)]);
        let frame = state.compose(140, 20).expect("frame");
        assert_eq!(
            backgrounds(&frame, rect),
            backgrounds(&plain, rect),
            "the focused tab and space keep their own background"
        );
    }

    state.handle_raw_events(vec![moved(other_space.x + 1, other_space.y)]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
    assert!(outcome.repaint, "leaving the window clears the highlight");
    let frame = state.compose(140, 20).expect("frame");
    assert_eq!(
        backgrounds(&frame, other_space),
        backgrounds(&plain, other_space)
    );
}
