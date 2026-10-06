use super::*;

use crate::terminal::TerminalRuntime;
use crate::workspace::Workspace;

type Render = std::sync::mpsc::Receiver<Vec<u8>>;

fn frame_symbol(surface: &crate::protocol::PaneSurfaceFrame, x: u16, y: u16) -> &str {
    let index = usize::from(y) * usize::from(surface.frame.width) + usize::from(x);
    &surface.frame.cells[index].symbol
}

fn set_unseen(server: &mut HeadlessServer, pane_id: crate::layout::PaneId) {
    server.app.state.workspaces[0]
        .pane_state_mut(pane_id)
        .unwrap()
        .seen = false;
}

fn seen(server: &HeadlessServer, pane_id: crate::layout::PaneId) -> bool {
    server.app.state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .seen
}

fn show_workspace(server: &mut HeadlessServer, workspace: Workspace, render: &Render) {
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.reconcile_client_shell_locations();
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    let _ = recv_pane_surface(render, "baseline");
}

/// A lone pane's frame is drawn in full renders, keeps its own output off the retained path,
/// and goes at its deadline.
fn lone_pane_frame_lasts_until_its_deadline(server: &mut HeadlessServer, render: &Render) {
    let pane_id = install_shared_view_test_runtime(server);
    server.render_and_stream();
    let baseline = recv_pane_surface(render, "baseline");
    let rect = baseline.panes[0].rect;
    assert_ne!(frame_symbol(&baseline, rect.x, rect.y), "┌");

    // Seeing a finished agent asks for a full render and wakes the loop for the frame's end.
    set_unseen(server, pane_id);
    assert!(server.app.state.mark_active_tab_seen());
    let now = Instant::now();
    assert!(server.handle_scheduled_tasks_headless(now, false));
    let until = server.app.state.next_seen_flash_deadline().unwrap();
    assert!(
        server
            .app
            .next_headless_loop_deadline_with_git_refresh(now, false, false)
            .unwrap()
            <= until
    );
    server.render_and_stream();
    let framed = recv_pane_surface(render, "framed");
    assert_eq!(frame_symbol(&framed, rect.x, rect.y), "┌");

    // The framed pane's own output would paint over the frame, so it isn't patched in.
    write_shared_test_pane(server, pane_id, b"more");
    assert!(!server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    server.render_and_stream();
    let _ = recv_pane_surface(render, "framed output");

    // The frame goes at its deadline, and output is patched in again.
    assert!(server.handle_scheduled_tasks_headless(until, false));
    server.render_and_stream();
    let unframed = recv_pane_surface(render, "unframed");
    assert_ne!(frame_symbol(&unframed, rect.x, rect.y), "┌");
    write_shared_test_pane(server, pane_id, b"again");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
}

/// Closing the tab in front of a finished agent shows it: once the window has focus, that
/// counts as looking at it.
fn closing_the_tab_in_front_counts_as_seeing_the_tab_behind(server: &mut HeadlessServer) {
    let now = Instant::now();
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let front = server.app.state.workspaces[0].test_add_tab(None);
    server.app.state.workspaces[0].switch_tab(front);
    set_unseen(server, pane_id);
    assert!(server.app.state.workspaces[0].close_tab(front));
    server.handle_scheduled_tasks_headless(now, false);
    assert!(!seen(server, pane_id));

    server.clients.get_mut(&7).unwrap().outer_terminal_focus = Some(true);
    assert!(server.handle_scheduled_tasks_headless(now, false));
    assert!(seen(server, pane_id));
    assert!(server.app.state.pane_has_seen_flash(0, pane_id));
}

/// The window's own tab is what it shows, even when the server's default tab is another one.
fn focused_window_sees_only_its_own_tab(server: &mut HeadlessServer, render: &Render) {
    let mut workspace = Workspace::test_new("two-tabs");
    let shown_tab = workspace.test_add_tab(None);
    let shown = workspace.tabs[shown_tab].root_pane;
    let default = workspace.tabs[0].root_pane;
    show_workspace(server, workspace, render);
    let shown_tab_id = server.app.public_tab_id(0, shown_tab).unwrap();
    assert!(server.focus_shell_client_on_tab(7, &shown_tab_id));
    assert_eq!(server.app.state.workspaces[0].active_tab_index(), 0);
    set_unseen(server, shown);
    set_unseen(server, default);

    server.handle_scheduled_tasks_headless(Instant::now(), false);

    assert!(seen(server, shown));
    assert!(server.app.state.pane_has_seen_flash(0, shown));
    assert!(!seen(server, default));
}

/// A bordered pane's frame is on its border, which its output never reaches, so its output
/// still takes the retained path.
fn bordered_pane_output_keeps_the_retained_path(server: &mut HeadlessServer, render: &Render) {
    let mut workspace = Workspace::test_new("split");
    let left = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.insert_test_runtime(left, TerminalRuntime::test_with_screen_bytes(40, 21, b"L"));
    workspace.insert_test_runtime(right, TerminalRuntime::test_with_screen_bytes(40, 21, b"R"));
    show_workspace(server, workspace, render);
    set_unseen(server, right);
    server.app.state.mark_active_tab_seen();
    assert!(server.handle_scheduled_tasks_headless(Instant::now(), false));
    server.render_and_stream();
    let _ = recv_pane_surface(render, "framed split");

    write_shared_test_pane(server, right, b"more");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([right])));
}

// One server for every case: test servers created in the same instant share a socket path.
#[tokio::test]
async fn seen_pane_frame_on_a_server() {
    let mut server = test_headless_server();
    let (_control, render) = connect_matching_test_shell(&mut server, 7);
    assert_eq!(server.foreground_client_id, Some(7));

    lone_pane_frame_lasts_until_its_deadline(&mut server, &render);
    closing_the_tab_in_front_counts_as_seeing_the_tab_behind(&mut server);
    focused_window_sees_only_its_own_tab(&mut server, &render);
    bordered_pane_output_keeps_the_retained_path(&mut server, &render);
    shutdown_test_runtimes(&mut server);
}
