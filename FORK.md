# herdr fork

A personal fork of [herdrdev/herdr](https://github.com/herdrdev/herdr). The `patches` branch is
the latest Herdr release with the changes below on top. Everything else is stock Herdr; see the
[README](README.md) and [herdr.dev/docs](https://herdr.dev/docs/).

## why sessions

Herdr is usually run as one session, with a space for each project. This fork is built around
one session per project instead, much like keeping one tmux session per project: each session
holds just that project's spaces and tabs. One window isn't cluttered with work from every
project, and two projects can each have a space called `workflow` without clashing.

Stock Herdr already runs several named sessions (`herdr --session <name>`), but a window shows
one at a time. Switching means detaching and attaching again, and the sidebar, agent counts and
notifications only cover the session you're attached to.

This fork lists every running session on the computer in the sidebar, the way Herdr lists SSH
machines, with each session's spaces below it. Clicking a session or one of its spaces switches
to it in the same window, and the red counts and notifications cover every session at once.

![Herdr with three sessions in the sidebar and red counts of waiting agents on its spaces](assets/fork-screenshot.png)

Three sessions in one window, each space showing how many agents are waiting for you. The 🟢 ⏳ 🟠
markers in the tab bar come from a separate Herdr plugin,
[Tab Status](https://github.com/bagonyi/herdr-tab-status), not from this fork.

## what's changed

### sessions in the sidebar

- Other running sessions on this computer are listed in the sidebar next to saved SSH machines.
  Click one to switch to it in the same window; its agents and notifications show up too. Only
  running named sessions are listed, not the default session.
- `herdr --session <name>` opens that session instead of the one you last selected.
- Sessions are sorted A–Z, so every window shows the same order. The `machine_order` setting
  puts chosen sessions or machines first.
- Sessions on this computer don't show the green "online" badge, since they're only listed while
  they run. The badge still appears when something is wrong.
- The list is headed "sessions" instead of "machines". A window opened on a named session
  shows the list even when its session is the only one running.
- A + after the "sessions" heading asks for a name and starts that session, as
  `herdr --session <name>` would, then switches to it. Its first space opens in your home folder.
  Spaces in the name become dashes, so "Test session" starts `Test-session`.
- A + on each session's row creates a space in that session, switching to it first if needed.
- Right-clicking another session offers "Stop session" (its spaces come back when you start it
  again with the + and its name) and "Delete session" (stops it and forgets its spaces). Both
  ask first.
- A named session ends when its last space is closed, like a tmux session, and is deleted;
  stock Herdr opens a fresh space instead. If its last shell was killed by a signal (say, at
  logout), it stops but keeps its spaces. The default session keeps the stock behaviour.
- A window showing its own session closes when that session is stopped, instead of waiting to
  reconnect. Live updates still reconnect.

### starting and stopping every session

- `herdr session stop --all` stops every running session at once. When run inside one of them,
  that session stops last.
- Plain `herdr` starts every saved session that isn't running, then opens the one you used last
  (or the first in the sidebar). It starts the default session only when no named session is
  saved. So after `herdr session stop --all`, or a restart of the computer, plain `herdr` brings
  everything back, and after detaching it reopens the session you were in.

### agents

- Each space shows a red count of agents waiting for you (finished and not yet looked at, or
  blocked), like an app badge.
- A window you open counts agents that finished while no window was looking, and any open
  questions, so closing and reopening it doesn't reset the counts.
- An agent that finishes in a session no window is showing counts as unseen until you look at it,
  even in the tab you last had open there. Stock Herdr counts that tab as looked at. Switching to
  another space in that session doesn't count either.
- The agents panel can be hidden, giving the spaces list the whole sidebar.
- Notifications leave out the 🟢 ⏳ 🟠 marker that [Tab Status](https://github.com/bagonyi/herdr-tab-status)
  puts in front of a tab's name. The plugin changes it only after the notification is made, so a
  "finished" notification would still show ⏳.

### mouse

- The tab, session or space row under the mouse gets a lighter background.
- The sidebar's dividing line lights up while the mouse is over it or dragging it.

## settings

Two settings are new; both go in `~/.config/herdr/config.toml`:

```toml
[ui.sidebar.spaces]
# Sessions or machines to list first, in this order; the rest keep their usual order.
machine_order = ["work", "personal"]

[ui.sidebar.agents]
# Hide the agents panel; the spaces list takes the whole sidebar.
hidden = true
```

## plugins

Two plugins that go well with this fork. Both work with stock Herdr too:

- [Tab Status](https://github.com/bagonyi/herdr-tab-status) puts 🟢 done, ⏳ working or 🟠 blocked
  in front of tab names, as in the screenshot above.
- [Restore Commands](https://github.com/bagonyi/herdr-restore-commands) brings back commands such
  as `gh dash` and `lnav` in their panes when a session restarts.

## building

There are no releases or binaries; build it from source. You need
[rustup](https://rustup.rs) (it installs the Rust version in `rust-toolchain.toml`) and
[Zig](https://ziglang.org/download/) 0.16.0, either on `PATH` or named by the `ZIG` variable.

```bash
git clone -b patches https://github.com/bagonyi/herdr
cd herdr
cargo build --release --locked
# then copy target/release/herdr somewhere on your PATH
```

Turn off Herdr's update check in `~/.config/herdr/config.toml`, and don't run `herdr update`:
both would replace the fork with stock Herdr.

```toml
[update]
version_check = false
```

## caveats

- Only built and tested on macOS on Apple silicon.
- The branch is rebased onto each new Herdr release and force-pushed, so `git pull` fails
  after an update. Use `git fetch` and `git reset --hard origin/patches` instead; this throws
  away any changes of your own.
- It still reports the upstream version number, for example `herdr 0.9.3`.

## license

Herdr is licensed under the [Apache License 2.0](LICENSE). This fork changes files under `src/`,
adds a note at the top of `README.md`, and adds this file. Each change is a separate commit on
top of a Herdr release tag. This fork doesn't carry the tags, so fetch them from Herdr to list
the commits and every changed file:

```bash
git remote add upstream https://github.com/herdrdev/herdr
git fetch upstream --tags
base=$(git describe --tags --abbrev=0 --match 'v[0-9]*' patches)
git log --oneline "$base"..patches
git diff --stat "$base"..patches
```
