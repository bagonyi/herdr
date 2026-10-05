//! Every saved session at once: plain `herdr` starts the saved sessions that aren't running and
//! opens the one used last, and `herdr session stop --all` stops every running session.

use std::path::PathBuf;

use crate::session::SessionInfo;

use super::endpoint::{ClientEndpointId, EndpointCatalog};

/// Holds the name of the session a window last showed.
fn last_session_path() -> PathBuf {
    crate::config::state_dir()
        .join("client")
        .join("last-session")
}

fn last_session() -> Option<String> {
    let name = std::fs::read_to_string(last_session_path()).ok()?;
    Some(name.trim().to_string())
}

fn remember(name: &str) {
    let path = last_session_path();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, name));
    if let Err(error) = written {
        tracing::warn!(%error, path = %path.display(), "failed to remember the last session");
    }
}

/// Notes that a window switched to `endpoint_id`, when it is a named session on this computer.
pub(crate) fn remember_endpoint(catalog: &EndpointCatalog, endpoint_id: &ClientEndpointId) {
    let name = match endpoint_id {
        ClientEndpointId::Local => crate::session::active_name(),
        ClientEndpointId::Ssh(id) => catalog
            .ssh
            .iter()
            .find(|profile| {
                &profile.id == id
                    && super::endpoint::is_local_session_profile(
                        profile.id.as_str(),
                        &profile.target,
                        &profile.session,
                    )
            })
            .map(|profile| profile.session.clone()),
    };
    if let Some(name) = name {
        remember(&name);
    }
}

/// Runs before a window opens. `herdr --session <name>` only notes the session as the last one
/// used. Plain `herdr` starts every saved named session that isn't running, in the background,
/// then opens the session used last, or else the first in the sidebar's order, as
/// `herdr --session <name>` would. With no saved named sessions, or a socket or session chosen
/// through the environment, Herdr opens the default session as before.
pub(crate) fn prepare_launch(machine_order: &[String]) {
    if crate::session::explicit_session_requested() {
        if let Some(name) = crate::session::active_name() {
            remember(&name);
        }
        return;
    }
    if std::env::var_os(crate::api::SOCKET_PATH_ENV_VAR).is_some()
        || std::env::var_os(crate::session::SESSION_ENV_VAR).is_some()
    {
        return;
    }
    let saved = match crate::session::list_sessions() {
        Ok(sessions) => sessions
            .into_iter()
            .filter(|session| !session.default)
            .collect::<Vec<_>>(),
        Err(error) => {
            tracing::warn!(%error, "failed to list saved sessions");
            return;
        }
    };
    let Some(open) = session_to_open(&saved, last_session().as_deref(), machine_order) else {
        return;
    };
    let open = open.to_string();
    for session in &saved {
        if session.running || session.name == open {
            continue;
        }
        // The window starts the session it opens itself, waiting until it's ready.
        if let Err(error) = super::shell::spawn_session_server(&session.name) {
            tracing::warn!(%error, session = %session.name, "failed to start saved session");
        }
    }
    // The same as `--session <name>`; valid, since it names a saved session.
    let args = ["herdr".to_string(), "--session".to_string(), open.clone()];
    if let Err(error) = crate::session::configure_from_args(&args) {
        tracing::warn!(%error, session = %open, "failed to open saved session");
        return;
    }
    remember(&open);
}

/// The session used last if it is still saved, else the first in the sidebar's order: labels in
/// `machine_order` first, then A-Z.
fn session_to_open<'a>(
    saved: &'a [SessionInfo],
    last: Option<&str>,
    machine_order: &[String],
) -> Option<&'a str> {
    if let Some(last) = saved
        .iter()
        .find(|session| Some(session.name.as_str()) == last)
    {
        return Some(&last.name);
    }
    saved
        .iter()
        .min_by_key(|session| {
            let listed = machine_order
                .iter()
                .position(|label| label == &session.name)
                .unwrap_or(machine_order.len());
            (listed, session.name.to_ascii_lowercase())
        })
        .map(|session| session.name.as_str())
}

/// `herdr session stop --all`: stops every running session at once. The session this command
/// runs in, if any, goes last, as stopping it ends this command too.
pub(crate) fn stop_all() -> std::io::Result<i32> {
    let running = crate::session::list_sessions()?
        .into_iter()
        .filter(|session| session.running)
        .collect::<Vec<_>>();
    if running.is_empty() {
        println!("no sessions running");
        return Ok(0);
    }
    let own_socket = std::env::var(crate::api::SOCKET_PATH_ENV_VAR).ok();
    let (own, others): (Vec<_>, Vec<_>) = running
        .into_iter()
        .partition(|session| Some(&session.socket_path) == own_socket.as_ref());

    // Each stop waits for its server to exit, so they run side by side.
    let stops = others
        .into_iter()
        .map(|session| std::thread::spawn(move || stop(&session)))
        .collect::<Vec<_>>();
    let mut failed = false;
    for stop in stops {
        failed |= !stop.join().unwrap_or(false);
    }
    for session in own {
        println!(
            "stopping session {}, which this command runs in",
            session.name
        );
        failed |= !stop(&session);
    }
    Ok(if failed { 1 } else { 0 })
}

fn stop(session: &SessionInfo) -> bool {
    let target = (!session.default).then_some(session.name.as_str());
    match crate::session::stop_session(target) {
        Ok(_) => {
            println!("stopped session {}", session.name);
            true
        }
        Err(error) => {
            eprintln!("failed to stop session {}: {error}", session.name);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(names: &[&str]) -> Vec<SessionInfo> {
        names
            .iter()
            .map(|name| SessionInfo {
                name: name.to_string(),
                default: false,
                running: false,
                socket_path: String::new(),
                session_dir: String::new(),
            })
            .collect()
    }

    #[test]
    fn opens_the_session_used_last() {
        let sessions = saved(&["Herdr", "SidelineDirector"]);
        assert_eq!(
            session_to_open(&sessions, Some("SidelineDirector"), &[]),
            Some("SidelineDirector")
        );
    }

    #[test]
    fn opens_the_first_in_sidebar_order_without_a_saved_last_session() {
        let sessions = saved(&["beta", "Alpha", "work"]);
        assert_eq!(session_to_open(&sessions, None, &[]), Some("Alpha"));
        assert_eq!(session_to_open(&sessions, Some("gone"), &[]), Some("Alpha"));
        let order = ["work".to_string()];
        assert_eq!(session_to_open(&sessions, None, &order), Some("work"));
    }

    #[test]
    fn opens_nothing_without_saved_sessions() {
        assert_eq!(session_to_open(&[], Some("Herdr"), &[]), None);
    }
}
