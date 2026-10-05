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
    assert!(state.take_pending_workspace_create().is_empty());

    click(&mut state, plus);
    assert!(state.activate_endpoint_projection(&other));
    let (endpoint_id, params) = created_workspace(&state.take_pending_workspace_create());
    assert_eq!(endpoint_id, other);
    assert_eq!(params.source_workspace_id.as_deref(), Some("ws_other"));
    assert!(state.take_pending_workspace_create().is_empty());
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
    let (endpoint_id, params) = created_workspace(&state.take_pending_workspace_create());
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
fn right_click_offers_stop_and_delete_for_other_sessions_only() {
    let (mut state, other) = state_with_session();
    state.compose(100, 28).expect("frame");
    let own = session_row(&state, &ClientEndpointId::Local);
    right_click(&mut state, own);
    assert!(
        state.overlay.is_none(),
        "no menu for the window's own session"
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
            !state.take_pending_workspace_create().is_empty(),
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
