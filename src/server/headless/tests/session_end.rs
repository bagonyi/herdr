use super::*;

use super::super::session_end::SessionEnd;

fn reset(server: &mut HeadlessServer, named: bool) {
    server.session_end = SessionEnd {
        named,
        had_spaces: false,
        ended: false,
        keep_saved_state: false,
    };
    server.app.state.should_quit = false;
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("one")];
}

// One server for every case: test servers created in the same instant share a socket path.
#[test]
fn only_a_named_session_that_had_spaces_ends_after_its_last_one() {
    let mut server = test_headless_server();

    // A named session that starts out empty still gets its first space.
    reset(&mut server, true);
    server.app.state.workspaces.clear();
    assert!(!server.end_session_after_last_space());
    assert!(!server.app.state.should_quit);

    reset(&mut server, true);
    assert!(!server.end_session_after_last_space());
    server.app.state.workspaces.clear();
    assert!(server.end_session_after_last_space());
    assert!(server.app.state.should_quit);
    assert!(server.delete_after_shutdown(), "closed by hand, so deleted");
    assert!(
        server.end_session_after_last_space(),
        "no replacement space while it shuts down"
    );

    // The default session keeps getting a fresh space.
    reset(&mut server, false);
    assert!(!server.end_session_after_last_space());
    server.app.state.workspaces.clear();
    assert!(!server.end_session_after_last_space());
    assert!(!server.app.state.should_quit);

    // A live update moves the spaces to the next server; it never ends the session.
    reset(&mut server, true);
    assert!(!server.end_session_after_last_space());
    server.handoff_in_progress = true;
    server.app.state.workspaces.clear();
    assert!(!server.end_session_after_last_space());
    assert!(!server.app.state.should_quit);
    assert!(!server.delete_after_shutdown());
}
