# Herdrsson

Herdrsson ("son of Herdr" in Icelandic) is a personal fork of
[herdrdev/herdr](https://github.com/herdrdev/herdr). It's unofficial: the Herdr project doesn't
make or endorse it. The `patches` branch is the latest Herdr release with the changes below on
top. Everything else is stock Herdr; see the [README](README.md) and
[herdr.dev/docs](https://herdr.dev/docs/).

It installs and runs as `herdr`, with Herdr's config folder, plugins and agent integrations, so
the `herdr` commands and `~/.config/herdr/config.toml` below are right as written.

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

![Herdrsson with three sessions in the sidebar and red counts of waiting agents on its spaces](assets/fork-screenshot.png)

Three sessions in one window, each space showing how many agents are waiting for you. The 🟢 ⏳ 🟠
markers in the tab bar come from a separate Herdr plugin,
[Tab Status](https://github.com/bagonyi/herdr-tab-status), not from this fork.

## what's changed

### sessions in the sidebar

- Other running sessions on this computer are listed in the sidebar next to saved SSH machines.
  Click one to switch to it in the same window; its agents and notifications show up too. Only
  named sessions are listed, not the default session.
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
- A ■ shows on a session's row while the mouse is over it, and stops the session, after asking.
  Stopping the session on screen switches the window to another running session first, so the
  window stays open while any other session runs.
- Stopped sessions are listed under a "stopped sessions" heading below the running ones, in any
  window that lists sessions.
  Clicking the heading folds them away (hovering it shows "hide" or "show"; folded, it shows how
  many there are). A ▶ on each starts it in the background, and clicking its name starts it and
  switches to it. Hovering a stopped session shows a trash can that deletes it, after asking.
- Right-clicking a session offers "Stop session" and "Delete session" (stops it and forgets its
  spaces), or "Start session" and "Delete session" for a stopped one. Stop and delete ask first.
- Right-clicking a space of another session switches to that session and opens the space's menu.
- Ctrl+click works as a right-click outside panes, as in other macOS apps. Inside panes it
  still goes to the program running there.
- A named session ends when its last space is closed, like a tmux session, and is deleted;
  stock Herdr opens a fresh space instead. If its last shell was killed by a signal (say, at
  logout), it stops but keeps its spaces. The default session keeps the stock behaviour.
- A window showing its own session closes when that session is stopped from elsewhere, such as
  `herdr session stop`, instead of waiting to reconnect. Live updates still reconnect.

### starting and stopping every session

- `herdr session stop --all` stops every running session at once. When run inside one of them,
  that session stops last.
- Plain `herdr` starts every saved session that isn't running, then opens the one you used last
  (or the first in the sidebar). It starts the default session only when no named session is
  saved. So after `herdr session stop --all`, or a restart of the computer, plain `herdr` brings
  everything back, and after detaching it reopens the session you were in.
- A session stopped on its own, with its ■, its menu or `herdr session stop <name>`, stays
  stopped: plain `herdr` leaves it out until you start it again.

### agents

- Each space shows a red count of agents waiting for you (finished and not yet looked at, or
  blocked), like an app badge.
- A window you open counts agents that finished while no window was looking, and any open
  questions, so closing and reopening it doesn't reset the counts.
- An agent that finishes in a session no window is showing counts as unseen until you look at it,
  even in the tab you last had open there. Stock Herdr counts that tab as looked at. Switching to
  another space in that session doesn't count either.
- When you look at an agent that finished, its pane gets a green frame for 2 seconds, or until the
  agent starts working again. A pane's border turns green; a pane without a border (a lone pane,
  by default) gets the frame drawn over its outermost cells, so the program in it isn't resized.
- A finished agent in a tab that comes on screen because the tab in front of it closed counts as
  looked at right away, as if you had switched to it. Stock Herdr waits until you next type or
  switch windows.
- The agents panel can be hidden, giving the spaces list the whole sidebar.
- Notifications leave out the 🟢 ⏳ 🟠 marker that [Tab Status](https://github.com/bagonyi/herdr-tab-status)
  puts in front of a tab's name. The plugin changes it only after the notification is made, so a
  "finished" notification would still show ⏳.

### mouse

- The tab, session or space row under the mouse gets a lighter background, and the sidebar's +, ■,
  ▶ and trash can buttons light up.
- The sidebar's dividing line lights up while the mouse is over it or dragging it.

### keys

- `last_tab` goes back to the tab you had open before this one in the same space, and
  `last_workspace` to the space you had open before this one, in any session. Press it again to
  come back. Each space remembers its own last tab; closed tabs and spaces are skipped. Both are
  unset by default.

### plugin panes

- A pane a plugin opens as a split or a tab gets its real size straight away. Stock Herdr starts
  it at the size of the pane it splits, so a full-screen program in it could stay drawn at that
  size, cut off at the edge, until you clicked it.
- A plugin can open a split at a set size: `herdr plugin pane open --placement split` (or
  `zoomed`) takes `--width 40%` for a split to the right or `--height 40%` for one below
  (`width` and `height` in `plugin.pane.open`), as a share of the pane being split, kept
  between 10% and 90%. Stock Herdr takes them only for popups, so a split opens at half and
  visibly jumps when the plugin resizes it.

## settings

Four settings are new; all go in `~/.config/herdr/config.toml`:

```toml
[keys]
# Back and forth between the two latest tabs of a space, and the two latest spaces.
last_tab = "alt+q"
last_workspace = "alt+w"

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

There are no prebuilt binaries; build it from source. You need
[rustup](https://rustup.rs) (it installs the Rust version in `rust-toolchain.toml`) and
[Zig](https://ziglang.org/download/) 0.16.0, either on `PATH` or named by the `ZIG` variable.

```bash
git clone -b patches https://github.com/bagonyi/herdrsson
cd herdrsson
cargo build --release --locked
# then copy target/release/herdr somewhere on your PATH
```

Turn off Herdr's update check in `~/.config/herdr/config.toml`, and don't run `herdr update`:
both would replace Herdrsson with stock Herdr.

```toml
[update]
version_check = false
```

## versions

`herdr --version` shows two numbers: Herdrsson's own version, which is the date it was
released, and the Herdr release it's built on.

```
$ herdr --version
herdrsson 2026.10.06 (based on herdr 0.9.3)
```

A second release on the same day ends in `.2`. Moving onto a new Herdr release changes only the
Herdr number. Anything that compares versions, such as a plugin's `min_herdr_version`, still
uses the Herdr number.

- 2026.10.06 (on Herdr 0.9.3): the first release as Herdrsson, with everything listed above.

## caveats

- Only built and tested on macOS on Apple silicon.
- The branch is rebased onto each new Herdr release and force-pushed, so `git pull` fails
  after an update. Use `git fetch` and `git reset --hard origin/patches` instead; this throws
  away any changes of your own.

## license

Herdr is licensed under the [Apache License 2.0](LICENSE). Herdr is the upstream project's name;
Herdrsson uses it only to say where it comes from. This fork changes files under `src/`, adds a
note at the top of `README.md`, and adds this file and `HERDRSSON_VERSION`. Each change is a separate commit on
top of a Herdr release tag. This fork doesn't carry the tags, so fetch them from Herdr to list
the commits and every changed file:

```bash
git remote add upstream https://github.com/herdrdev/herdr
git fetch upstream --tags
base=$(git describe --tags --abbrev=0 --match 'v[0-9]*' patches)
git log --oneline "$base"..patches
git diff --stat "$base"..patches
```
