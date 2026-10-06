use super::*;
use crate::api::schema::WorkspaceCreateParams;
use crate::client::endpoint::{local_session_profile_id, ClientEndpointStatus, SavedSshEndpoint};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

fn session_profile(name: &str) -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: local_session_profile_id(name),
        label: name.into(),
        target: crate::client::endpoint::LOCAL_SESSION_TARGET.into(),
        session: name.into(),
        enabled: true,
    }
}

/// This window's own session plus a second running session, "Other", with a focused space.
fn state_with_session() -> (ClientShellState, ClientEndpointId) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let profile = session_profile("Other");
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut other = snapshot();
    other.boot_id = "other-boot".into();
    other.workspaces[0].workspace_id = "ws_other".into();
    other.workspaces[0].label = "other-space".into();
    state.set_endpoint_snapshot(&endpoint_id, Box::new(other));
    (state, endpoint_id)
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + rect.width / 2,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    })])
}

fn space_plus(state: &ClientShellState, endpoint_id: &ClientEndpointId) -> Rect {
    state
        .hits
        .new_session_workspace
        .iter()
        .find(|(_, id)| id == endpoint_id)
        .map(|(rect, _)| *rect)
        .expect("session row has a +")
}

fn created_workspace(actions: &[ClientShellAction]) -> (ClientEndpointId, WorkspaceCreateParams) {
    match actions {
        [ClientShellAction::Endpoint {
            endpoint_id,
            request,
            ..
        }] => match &request.method {
            crate::api::schema::Method::WorkspaceCreate(params) => {
                (endpoint_id.clone(), params.clone())
            }
            other => panic!("expected workspace.create, got {other:?}"),
        },
        other => panic!("expected one endpoint request, got {other:?}"),
    }
}

#[test]
fn sessions_heading_and_each_session_row_show_a_plus() {
    let (mut state, other) = state_with_session();
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let heading = state.hits.new_session;
    assert!(!heading.is_empty());
    assert_eq!(buffer[(heading.x + 1, heading.y)].symbol(), "+");
    for endpoint_id in [ClientEndpointId::Local, other] {
        let plus = space_plus(&state, &endpoint_id);
        assert_eq!(buffer[(plus.x + 1, plus.y)].symbol(), "+");
        assert_eq!(plus.x, heading.x, "row + lines up with the heading +");
    }
}

#[test]
fn plus_buttons_need_mouse_capture() {
    let (mut state, _) = state_with_session();
    state.config.mouse_capture = false;
    state.compose(100, 28).expect("frame");
    assert!(state.hits.new_session.is_empty());
    assert!(state.hits.new_session_workspace.is_empty());
}

#[test]
fn plus_after_sessions_heading_asks_for_a_name_and_starts_the_session() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    let heading = state.hits.new_session;
    click(&mut state, heading);
    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("expected the name popup");
    };
    assert_eq!(rename.title, "new session");
    assert!(state.insert_overlay_text("web-app"));
    let mut outcome = ClientShellInput::default();
    state.save_rename_overlay(&mut outcome);
    assert!(state.overlay.is_none());
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StartSession { name }] if name == "web-app"
    ));
}

#[test]
fn started_session_opens_once_it_is_online_or_reports_a_timeout() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    for name in ["web-app", "slow"] {
        let heading = state.hits.new_session;
        click(&mut state, heading);
        assert!(state.insert_overlay_text(name));
        state.save_rename_overlay(&mut ClientShellInput::default());
    }
    // Only the latest request is followed.
    let now = std::time::Instant::now();
    assert_eq!(state.take_started_session(now), None);
    assert!(state.visible_endpoint_notice.is_none());
    let endpoint_id = ClientEndpointId::Ssh(local_session_profile_id("slow"));
    state.set_endpoint_catalog(&[session_profile("Other"), session_profile("slow")]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    assert_eq!(state.take_started_session(now), None, "no snapshot yet");
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));
    assert_eq!(state.take_started_session(now), Some(endpoint_id));
    assert_eq!(state.take_started_session(now), None);

    let heading = state.hits.new_session;
    click(&mut state, heading);
    assert!(state.insert_overlay_text("never"));
    state.save_rename_overlay(&mut ClientShellInput::default());
    let later = now + std::time::Duration::from_secs(31);
    assert_eq!(state.take_started_session(later), None);
    let notice = state
        .visible_endpoint_notice
        .take()
        .expect("timeout notice");
    assert!(notice.body.contains("never"));
    assert_eq!(state.take_started_session(later), None);
    assert!(state.visible_endpoint_notice.is_none());
}

#[test]
fn typed_session_names_become_valid_ones() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    for (typed, expected) in [
        ("Test session", "Test-session"),
        ("  my   new app ", "my-new-app"),
        ("café #2", "caf-2"),
    ] {
        let heading = state.hits.new_session;
        click(&mut state, heading);
        assert!(state.insert_overlay_text(typed));
        let mut outcome = ClientShellInput::default();
        state.save_rename_overlay(&mut outcome);
        assert!(
            matches!(
                outcome.actions.as_slice(),
                [ClientShellAction::StartSession { name }] if name == expected
            ),
            "{typed}"
        );
        assert!(state.overlay.is_none());
    }
}

#[test]
fn unusable_or_listed_session_names_do_not_start_a_server() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    for (name, rejected) in [("###", true), ("default", true), ("Other", false)] {
        let heading = state.hits.new_session;
        click(&mut state, heading);
        assert!(state.insert_overlay_text(name));
        let mut outcome = ClientShellInput::default();
        state.save_rename_overlay(&mut outcome);
        assert!(
            !outcome
                .actions
                .iter()
                .any(|action| matches!(action, ClientShellAction::StartSession { .. })),
            "{name}"
        );
        assert_eq!(
            state.visible_endpoint_notice.take().is_some(),
            rejected,
            "{name}"
        );
        if rejected {
            // The popup stays open with the name as typed, ready to be corrected.
            let Some(ClientShellOverlay::Rename(rename)) = state.overlay.take() else {
                panic!("{name}: expected the popup to stay open");
            };
            assert_eq!(rename.title, "new session");
            assert_eq!(rename.input.as_str(), name);
        } else {
            assert!(state.overlay.is_none());
            assert!(matches!(
                outcome.actions.as_slice(),
                [ClientShellAction::ActivateEndpoint { endpoint_id, target: None }]
                    if endpoint_id == &other
            ));
        }
    }
}

#[test]
fn plus_on_the_shown_session_creates_a_space_there() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    let plus = space_plus(&state, &ClientEndpointId::Local);
    let outcome = click(&mut state, plus);
    let (endpoint_id, params) = created_workspace(&outcome.actions);
    assert_eq!(endpoint_id, ClientEndpointId::Local);
    assert_eq!(params.source_workspace_id.as_deref(), Some("ws_1"));
    assert!(params.focus);
}

#[test]
fn plus_on_another_session_switches_to_it_then_creates_the_space() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let plus = space_plus(&state, &other);
    let outcome = click(&mut state, plus);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id, target: None }] if endpoint_id == &other
    ));
    assert!(!state.collapsed_endpoints.contains(&other));
    // A switch that ends on another session drops the request.
    assert!(state.take_after_switch().is_empty());

    click(&mut state, plus);
    assert!(state.activate_endpoint_projection(&other));
    let (endpoint_id, params) = created_workspace(&state.take_after_switch());
    assert_eq!(endpoint_id, other);
    assert_eq!(params.source_workspace_id.as_deref(), Some("ws_other"));
    assert!(state.take_after_switch().is_empty());
}

#[test]
fn plus_on_another_session_with_name_prompt_asks_first() {
    let (mut state, other) = state_with_session();
    state.config.prompt_new_workspace_name = true;
    state.compose(100, 28).expect("frame");
    let plus = space_plus(&state, &other);
    let outcome = click(&mut state, plus);
    assert!(
        outcome.actions.is_empty(),
        "no switch until the name is saved"
    );
    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_mut() else {
        panic!("expected the name popup");
    };
    assert_eq!(rename.title, "new workspace");
    rename.input.clear();
    assert!(state.insert_overlay_text("notes"));
    let mut outcome = ClientShellInput::default();
    state.save_rename_overlay(&mut outcome);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id, target: None }] if endpoint_id == &other
    ));
    assert!(state.activate_endpoint_projection(&other));
    let (endpoint_id, params) = created_workspace(&state.take_after_switch());
    assert_eq!(endpoint_id, other);
    assert_eq!(params.label.as_deref(), Some("notes"));
}

fn right_click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 3,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    })])
}

fn session_row(state: &ClientShellState, endpoint_id: &ClientEndpointId) -> Rect {
    state
        .hits
        .machines
        .iter()
        .find(|hit| &hit.endpoint_id == endpoint_id)
        .map(|hit| hit.rect)
        .expect("session row")
}

#[test]
fn right_click_offers_stop_and_delete_for_sessions_but_not_the_default_one() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let own = session_row(&state, &ClientEndpointId::Local);
    right_click(&mut state, own);
    assert!(
        state.overlay.is_none(),
        "no menu for a window on the default session"
    );

    let row = session_row(&state, &other);
    right_click(&mut state, row);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("expected the session menu");
    };
    let labels = menu
        .items()
        .iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert_eq!(labels, ["Stop session", "Delete session"]);
}

#[test]
fn stopping_or_deleting_a_session_asks_first() {
    for (index, delete) in [(0, false), (1, true)] {
        let (mut state, other) = state_with_session();
        state.compose(100, 28).expect("frame");
        let row = session_row(&state, &other);
        right_click(&mut state, row);
        let mut outcome = ClientShellInput::default();
        state.activate_context_menu_item(index, &mut outcome);
        assert!(
            outcome.actions.is_empty(),
            "nothing stops before confirming"
        );
        let Some(ClientShellOverlay::ConfirmClose(confirm)) = state.overlay.as_ref() else {
            panic!("expected a confirmation");
        };
        assert!(confirm.title.starts_with(if delete {
            "Delete session?"
        } else {
            "Stop session?"
        }));
        assert_eq!(confirm.detail, "Other — 1 workspace");
        let mut outcome = ClientShellInput::default();
        state.accept_close_confirmation(&mut outcome);
        assert!(matches!(
            outcome.actions.as_slice(),
            [ClientShellAction::StopSession { name, delete: deleted }]
                if name == "Other" && *deleted == delete
        ));
    }
}

#[test]
fn a_named_session_lists_sessions_even_when_alone() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_named_session(true);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let heading = (0..20)
        .map(|x| buffer[(x, 0)].symbol().to_owned())
        .collect::<String>();
    assert!(heading.starts_with(" sessions"), "{heading:?}");
    assert_eq!(state.hits.machines.len(), 1);
    assert!(!state.hits.new_session.is_empty());
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_1"));

    let space = state.hits.workspaces[0].rect;
    let outcome = state.handle_raw_events(
        [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ]
        .into_iter()
        .map(|kind| {
            RawInputEvent::Mouse(MouseEvent {
                kind,
                column: space.x + 4,
                row: space.y,
                modifiers: KeyModifiers::NONE,
            })
        })
        .collect(),
    );
    assert!(
        matches!(
            outcome.actions.as_slice(),
            [ClientShellAction::Endpoint { endpoint_id: ClientEndpointId::Local, request, .. }]
                if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(_))
        ),
        "clicking a space of the only session focuses it without switching sessions"
    );

    state.set_named_session(false);
    state.compose(100, 28).expect("frame");
    assert!(state.hits.machines.is_empty(), "stock sidebar otherwise");
}

#[test]
fn a_space_waiting_for_a_switch_is_dropped_when_the_user_goes_elsewhere() {
    for (switch, targeted, kept) in [
        (true, false, true),
        (true, true, false),
        (false, false, false),
    ] {
        let (mut state, other) = state_with_session();
        state.compose(100, 28).expect("frame");
        let plus = space_plus(&state, &other);
        click(&mut state, plus);
        let requested = if switch {
            other.clone()
        } else {
            ClientEndpointId::Local
        };
        state.endpoint_activation_requested(&requested, targeted);
        assert!(state.activate_endpoint_projection(&other));
        assert_eq!(
            !state.take_after_switch().is_empty(),
            kept,
            "switch to the session: {switch}, to a chosen space: {targeted}"
        );
    }
}

#[test]
fn a_started_session_waits_for_open_popups_and_yields_to_other_switches() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    let heading = state.hits.new_session;
    click(&mut state, heading);
    assert!(state.insert_overlay_text("web"));
    state.save_rename_overlay(&mut ClientShellInput::default());
    let endpoint_id = ClientEndpointId::Ssh(local_session_profile_id("web"));
    state.set_endpoint_catalog(&[session_profile("Other"), session_profile("web")]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));

    let now = std::time::Instant::now();
    state.open_rename_workspace_overlay();
    assert!(state.overlay.is_some());
    assert!(state.poll_sessions(now).0.is_empty(), "waits for the popup");
    state.overlay = None;
    state.endpoint_activation_requested(&ClientEndpointId::Local, false);
    assert!(
        state.poll_sessions(now).0.is_empty(),
        "the user switched elsewhere"
    );
}

#[test]
fn escape_on_a_session_confirmation_only_closes_it() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let row = session_row(&state, &other);
    right_click(&mut state, row);
    state.activate_context_menu_item(0, &mut ClientShellInput::default());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ConfirmClose(_))
    ));
    let mode = state.mode;
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Esc,
        KeyModifiers::NONE,
    ))]);
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, mode);
}

#[test]
fn a_failed_stop_replacing_another_notice_is_repainted() {
    let (mut state, _) = state_with_session();
    state.receive_endpoint_unavailable("something else".into());
    let stop = std::thread::spawn(|| Err("boom".to_owned()));
    while !stop.is_finished() {
        std::thread::yield_now();
    }
    state.track_session_stop("web".into(), Ok(stop));
    let (actions, repaint) = state.poll_sessions(std::time::Instant::now());
    assert!(actions.is_empty());
    assert!(repaint);
    let notice = state.visible_endpoint_notice.as_ref().expect("stop notice");
    assert!(notice.body.contains("web") && notice.body.contains("boom"));
    assert!(!state.poll_sessions(std::time::Instant::now()).1);
}

fn menu_labels(state: &ClientShellState) -> Vec<&'static str> {
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("expected a menu");
    };
    menu.items().iter().map(|item| item.label).collect()
}

fn row_text(buffer: &Buffer, y: u16, width: u16) -> String {
    (0..width)
        .map(|x| buffer[(x, y)].symbol().to_owned())
        .collect()
}

#[test]
fn stopped_sessions_show_under_their_own_heading_with_a_play_button() {
    let (mut state, _) = state_with_session();
    assert!(stopped(&mut state, &["Archive"]));
    assert!(!stopped(&mut state, &["Archive"]));
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let heading = state.hits.stopped_header;
    assert!(row_text(&buffer, heading.y, 20).starts_with(" stopped sessions"));
    let [hit] = state.hits.stopped_sessions.as_slice() else {
        panic!("one stopped session");
    };
    assert_eq!(hit.name, "Archive");
    assert_eq!(
        hit.rect.y,
        heading.y + 2,
        "a gap under the heading, as under \"sessions\""
    );
    assert!(row_text(&buffer, hit.rect.y, 20).starts_with("   Archive"));
    assert_eq!(buffer[(hit.play.x + 1, hit.play.y)].symbol(), "▶");
    assert_eq!(
        hit.play.x, state.hits.new_session.x,
        "play lines up with the + buttons"
    );

    let play = hit.play;
    let outcome = click(&mut state, play);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StartSession { name }] if name == "Archive"
    ));
    assert!(
        click(&mut state, play).actions.is_empty(),
        "a session already starting isn't started twice"
    );
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    assert_eq!(buffer[(play.x + 1, play.y)].symbol(), "…");
    assert!(stopped(&mut state, &[]));
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    assert!(state.hits.stopped_sessions.is_empty());
    assert!(!(0..28).any(|y| row_text(&buffer, y, 20).contains("stopped sessions")));
}

#[test]
fn the_stopped_sessions_heading_folds_them_away() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["Archive", "Old"]);
    state.compose(100, 28).expect("frame");
    let heading = state.hits.stopped_header;
    click(&mut state, heading);
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    assert!(state.hits.stopped_sessions.is_empty());
    let text = row_text(&buffer, heading.y, heading.right());
    assert!(text.starts_with(" stopped sessions "), "{text:?}");
    assert!(
        text.ends_with(" 2 "),
        "the count, in the + column: {text:?}"
    );
    click(&mut state, heading);
    state.compose(100, 28).expect("frame");
    assert_eq!(state.hits.stopped_sessions.len(), 2);
}

#[test]
fn clicking_a_stopped_session_starts_it_and_opens_it_once_online() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["web"]);
    state.compose(100, 28).expect("frame");
    let name = state.hits.stopped_sessions[0].rect;
    let outcome = click(&mut state, Rect::new(name.x + 3, name.y, 1, 1));
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StartSession { name }] if name == "web"
    ));
    let endpoint_id = ClientEndpointId::Ssh(local_session_profile_id("web"));
    state.set_endpoint_catalog(&[session_profile("Other"), session_profile("web")]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));
    assert!(matches!(
        state.poll_sessions(std::time::Instant::now()).0.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id: id, .. }] if *id == endpoint_id
    ));
}

#[test]
fn right_click_on_a_stopped_session_offers_start_and_delete() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["Archive"]);
    state.compose(100, 28).expect("frame");
    let row = state.hits.stopped_sessions[0].rect;
    right_click(&mut state, row);
    assert_eq!(menu_labels(&state), ["Start session", "Delete session"]);
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(0, &mut outcome);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StartSession { name }] if name == "Archive"
    ));

    right_click(&mut state, row);
    state.activate_context_menu_item(1, &mut ClientShellInput::default());
    let mut outcome = ClientShellInput::default();
    state.accept_close_confirmation(&mut outcome);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StopSession { name, delete: true }] if name == "Archive"
    ));
}

#[test]
fn each_other_session_has_a_stop_button_that_asks_first() {
    let (mut state, other) = state_with_session();
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let [(stop, endpoint_id)] = state.hits.session_stop.as_slice() else {
        panic!("only the named session has a stop button, not the default one");
    };
    assert_eq!(endpoint_id, &other);
    assert_eq!(
        buffer[(stop.x + 1, stop.y)].symbol(),
        " ",
        "shown only while the row is hovered"
    );
    let stop = *stop;
    let buffer = hover(&mut state, stop.x - 6, stop.y);
    assert_eq!(buffer[(stop.x + 1, stop.y)].symbol(), "■");
    assert_eq!(stop.x + 2, space_plus(&state, &other).x, "left of the +");
    assert!(click(&mut state, stop).actions.is_empty());
    let Some(ClientShellOverlay::ConfirmClose(confirm)) = state.overlay.as_ref() else {
        panic!("expected a confirmation");
    };
    assert!(confirm.title.starts_with("Stop session?"));
    let mut outcome = ClientShellInput::default();
    state.accept_close_confirmation(&mut outcome);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StopSession { name, delete: false }] if name == "Other"
    ));
}

#[test]
fn stopping_the_shown_session_switches_to_another_first() {
    let (mut state, other) = state_with_session();
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    assert!(state.activate_endpoint_projection(&other));
    state.compose(100, 28).expect("frame");
    let stop = state.hits.session_stop[0].0;
    hover(&mut state, stop.x + 1, stop.y);
    click(&mut state, stop);
    let mut outcome = ClientShellInput::default();
    state.accept_close_confirmation(&mut outcome);
    assert!(
        matches!(
            outcome.actions.as_slice(),
            [ClientShellAction::ActivateEndpoint {
                endpoint_id: ClientEndpointId::Local,
                ..
            }]
        ),
        "{:?}",
        outcome.actions
    );
    state.endpoint_activation_requested(&ClientEndpointId::Local, false);
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert!(matches!(
        state.take_after_switch().as_slice(),
        [ClientShellAction::StopSession { name, delete: false }] if name == "Other"
    ));
}

#[test]
fn right_click_on_another_sessions_space_switches_then_opens_its_menu() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let space = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == other)
        .map(|hit| hit.rect)
        .expect("other session's space");
    let outcome = right_click(&mut state, space);
    assert!(state.overlay.is_none(), "the menu waits for the switch");
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id, .. }] if *endpoint_id == other
    ));
    state.endpoint_activation_requested(&other, false);
    assert!(state.activate_endpoint_projection(&other));
    assert!(state.take_after_switch().is_empty());
    assert!(state.overlay.is_none(), "opens once the switch is drawn");
    state.compose(100, 28).expect("frame");
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_other")
        .map(|hit| hit.rect)
        .expect("the space's row after the switch");
    assert!(state.poll_sessions(std::time::Instant::now()).1, "repaints");
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("expected the space's menu");
    };
    assert!(matches!(
        &menu.target,
        ClientContextMenuTarget::Workspace { workspace_id, .. } if workspace_id == "ws_other"
    ));
    assert_eq!(menu.y, row.y, "beside the space's row");
}

#[test]
fn a_space_menu_waiting_for_a_switch_yields_to_a_popup_opened_meanwhile() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let space = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == other)
        .map(|hit| hit.rect)
        .expect("other session's space");
    right_click(&mut state, space);
    state.endpoint_activation_requested(&other, false);
    assert!(state.activate_endpoint_projection(&other));
    state.take_after_switch();
    state.compose(100, 28).expect("frame");
    let heading = state.hits.new_session;
    click(&mut state, heading);
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Rename(_))));
    state.poll_sessions(std::time::Instant::now());
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Rename(_))),
        "the popup stays"
    );
    state.overlay = None;
    state.poll_sessions(std::time::Instant::now());
    assert!(state.overlay.is_none(), "and the menu was dropped");
}

#[test]
fn ctrl_click_is_a_right_click_outside_panes() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let row = session_row(&state, &other);
    let ctrl_click = |state: &mut ClientShellState, column, row| {
        state.handle_raw_events(
            [
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::Up(MouseButton::Left),
            ]
            .into_iter()
            .map(|kind| {
                RawInputEvent::Mouse(MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::CONTROL,
                })
            })
            .collect(),
        )
    };
    let outcome = ctrl_click(&mut state, row.x + 3, row.y);
    assert!(outcome.actions.is_empty(), "no switch to the session");
    assert_eq!(menu_labels(&state), ["Stop session", "Delete session"]);
    state.overlay = None;

    let pane = state.hits.panes[0].inner_rect;
    ctrl_click(&mut state, pane.x + 2, pane.y + 1);
    assert!(state.overlay.is_none(), "panes get ctrl+click as it is");
}

/// Moves the mouse to a cell and returns the frame drawn afterwards.
fn hover(state: &mut ClientShellState, column: u16, row: u16) -> Buffer {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })]);
    let frame = state.compose(100, 28).expect("frame");
    frame.to_ratatui_buffer().expect("frame should reconstruct")
}

#[test]
fn a_hovered_stopped_session_is_highlighted_and_shows_its_delete_button() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["Archive"]);
    state.compose(100, 28).expect("frame");
    let hit = &state.hits.stopped_sessions[0];
    let (row, delete, play) = (hit.rect, hit.delete, hit.play);
    assert_eq!(
        delete.x + 3,
        play.x,
        "a two-cell icon one cell clear of the play button"
    );
    let palette = state.config.palette.clone();

    let buffer = hover(&mut state, row.x + 4, row.y);
    assert_eq!(buffer[(row.x + 4, row.y)].bg, palette.selection_bg);
    assert_eq!(buffer[(delete.x + 1, delete.y)].symbol(), "\u{f1f8}");
    assert_ne!(buffer[(play.x + 1, play.y)].fg, palette.text);

    let buffer = hover(&mut state, play.x + 1, play.y);
    assert_eq!(
        buffer[(play.x + 1, play.y)].fg,
        palette.text,
        "the button under the mouse lights up"
    );

    let buffer = hover(&mut state, row.x + 4, row.y + 3);
    assert_eq!(buffer[(delete.x + 1, delete.y)].symbol(), " ");

    hover(&mut state, delete.x + 1, delete.y);
    assert!(click(&mut state, delete).actions.is_empty());
    let Some(ClientShellOverlay::ConfirmClose(confirm)) = state.overlay.as_ref() else {
        panic!("expected a confirmation");
    };
    assert!(confirm.title.starts_with("Delete session?"));
    let mut outcome = ClientShellInput::default();
    state.accept_close_confirmation(&mut outcome);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::StopSession { name, delete: true }] if name == "Archive"
    ));
}

#[test]
fn hovering_the_stopped_sessions_heading_offers_to_hide_or_show_them() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["Archive"]);
    state.compose(100, 28).expect("frame");
    let heading = state.hits.stopped_header;
    let label = |buffer: &Buffer| row_text(buffer, heading.y, heading.right());
    assert!(!label(&hover(&mut state, 0, 0)).contains("hide"));
    assert!(label(&hover(&mut state, heading.x + 3, heading.y)).ends_with(" hide "));
    click(&mut state, heading);
    assert!(label(&hover(&mut state, heading.x + 3, heading.y)).ends_with(" show "));
}

/// Tells the window which sessions are stopped, its own one running. Returns whether the
/// sidebar changed.
fn stopped(state: &mut ClientShellState, names: &[&str]) -> bool {
    state
        .set_saved_sessions(crate::client::saved_sessions::SavedSessions {
            stopped: names.iter().map(|name| name.to_string()).collect(),
            own_running: true,
        })
        .0
}

#[test]
fn a_start_that_never_comes_online_gets_its_play_button_back() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["Archive"]);
    state.compose(100, 28).expect("frame");
    let play = state.hits.stopped_sessions[0].play;
    click(&mut state, play);
    state.compose(100, 28).expect("frame");
    assert!(state.hits.stopped_sessions[0].play.is_empty(), "starting");
    assert!(
        state.hits.stopped_sessions[0].delete.is_empty(),
        "a session that is starting can't be deleted"
    );

    let later = std::time::Instant::now() + std::time::Duration::from_secs(31);
    let (_, repaint) = state.poll_sessions(later);
    assert!(repaint);
    let notice = state.visible_endpoint_notice.as_ref().expect("a notice");
    assert!(notice.body.contains("Archive"), "{}", notice.body);
    state.compose(100, 28).expect("frame");
    let play = state.hits.stopped_sessions[0].play;
    assert!(!play.is_empty(), "the play button is back");
    assert!(matches!(
        click(&mut state, play).actions.as_slice(),
        [ClientShellAction::StartSession { .. }]
    ));
}

#[test]
fn clicking_a_starting_sessions_name_opens_it_once_online() {
    let (mut state, _) = state_with_session();
    stopped(&mut state, &["web"]);
    state.compose(100, 28).expect("frame");
    let hit = &state.hits.stopped_sessions[0];
    let (play, row) = (hit.play, hit.rect);
    click(&mut state, play);
    state.compose(100, 28).expect("frame");
    let outcome = click(&mut state, Rect::new(row.x + 3, row.y, 1, 1));
    assert!(outcome.actions.is_empty(), "not started twice");
    let endpoint_id = ClientEndpointId::Ssh(local_session_profile_id("web"));
    state.set_endpoint_catalog(&[session_profile("Other"), session_profile("web")]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));
    assert!(matches!(
        state.poll_sessions(std::time::Instant::now()).0.as_slice(),
        [ClientShellAction::ActivateEndpoint { endpoint_id: id, .. }] if *id == endpoint_id
    ));
}

#[test]
fn the_windows_own_session_is_left_out_while_it_isnt_running() {
    let (mut state, other) = state_with_session();
    let saved = |own_running| crate::client::saved_sessions::SavedSessions {
        stopped: Vec::new(),
        own_running,
    };
    // Stopped or deleted: no longer in the stopped list either way.
    assert_eq!(state.set_saved_sessions(saved(false)), (true, false));
    state.compose(100, 28).expect("frame");
    let shown = state
        .hits
        .machines
        .iter()
        .map(|hit| hit.endpoint_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(shown, [other]);
    assert_eq!(
        state.set_saved_sessions(saved(true)),
        (true, true),
        "running again: reconnect now"
    );
    state.compose(100, 28).expect("frame");
    assert_eq!(state.hits.machines.len(), 2);
}

#[test]
fn a_stop_button_works_only_while_its_row_is_hovered() {
    let (mut state, _) = state_with_session();
    state.compose(100, 28).expect("frame");
    let stop = state.hits.session_stop[0].0;
    click(&mut state, stop);
    assert!(state.overlay.is_none(), "hidden, so nothing happens");
    hover(&mut state, stop.x + 1, stop.y);
    click(&mut state, stop);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ConfirmClose(_))
    ));
}

/// The window shows "Other"; stopping it waits for the switch to the window's own session.
fn stop_shown_session(state: &mut ClientShellState, other: &ClientEndpointId) {
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    assert!(state.activate_endpoint_projection(other));
    state.compose(100, 28).expect("frame");
    let stop = state.hits.session_stop[0].0;
    hover(state, stop.x + 1, stop.y);
    click(state, stop);
    state.accept_close_confirmation(&mut ClientShellInput::default());
}

#[test]
fn a_stop_waiting_for_a_switch_gives_up_with_a_notice() {
    let (mut state, other) = state_with_session();
    stop_shown_session(&mut state, &other);
    let later = std::time::Instant::now() + std::time::Duration::from_secs(11);
    assert!(state.poll_sessions(later).1);
    let notice = state.visible_endpoint_notice.as_ref().expect("a notice");
    assert!(notice.body.contains("Other"), "{}", notice.body);
    state.endpoint_activation_requested(&ClientEndpointId::Local, false);
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert!(
        state.take_after_switch().is_empty(),
        "a later switch doesn't stop it"
    );
}

#[test]
fn a_stop_waiting_for_a_switch_outlasts_other_work_waiting_for_one() {
    let (mut state, other) = state_with_session();
    stop_shown_session(&mut state, &other);
    // A space of the window's own session asks for its menu before the switch finishes.
    let space = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == ClientEndpointId::Local)
        .map(|hit| hit.rect)
        .expect("own session's space");
    right_click(&mut state, space);
    state.endpoint_activation_requested(&ClientEndpointId::Local, false);
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert!(matches!(
        state.take_after_switch().as_slice(),
        [ClientShellAction::StopSession { name, delete: false }] if name == "Other"
    ));
}
