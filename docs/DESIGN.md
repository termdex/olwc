# OpenLook-for-Wayland — Design Doc (v0.1)

## Goal

Recreate the *spirit* of OpenLook (olwm/olvwm) — the OPEN LOOK window
manager from SunOS/OpenWindows — as a modern Wayland compositor. Not a
pixel-perfect clone, not a port of the original codebase: a
recognizable homage built on current architecture, with long-term
maintainability and cross-platform (Linux + FreeBSD) support as
first-class goals.

## Non-goals

- Bit-for-bit visual or behavioral fidelity to olwm/olvwm.
- Reimplementing the pannable Virtual Desktop Manager (VDM). Replaced
  with discrete, linear, swipeable workspaces.
- Source compatibility with the original XView/olwm codebase. The
  original is a behavioral/visual reference only (clean-room
  reimplementation).

## High-level architecture

Two separate processes, split along a privilege boundary:

```
┌─────────────────────────┐        ┌──────────────────────────┐
│   olcore (compositor)    │        │   olshell (shell)         │
│   — C, wlroots            │◄──────►│   — Rust, Wayland client   │
│   — privileged             │  IPC   │   — unprivileged            │
│   — DRM/KMS, libinput,      │ proto  │   — panels, menus,           │
│     seat mgmt, protocol     │        │     workspace switcher,       │
│     compositing              │        │     theming/decoration UI      │
└─────────────────────────┘        └──────────────────────────┘
```

**olcore** is the compositor proper: talks to the kernel/hardware
(DRM/KMS, libinput), owns the seat, implements core Wayland protocol
and compositing, and arbitrates window placement/focus. Built on
wlroots for a proven, portable backend (including FreeBSD, via
precedent like hikari).

**olshell** is an ordinary Wayland client (privileged only in the UI
sense, not the kernel sense) that renders everything
OpenLook-specific: the pushpin-style menus, window gadgets/decoration
chrome, workspace switcher strip, and root-window background/menu.
It talks to olcore purely over standard and custom Wayland protocol
extensions — no shared memory, no special IPC channel, no elevated
privileges.

### Why this split

- **Security/robustness containment.** The highest-risk code
  (buffer lifetimes, input handling, client teardown) stays in a
  small, auditable core. A shell crash doesn't take down the whole
  session.
- **Language fit.** C+wlroots gets proven FreeBSD support for the
  privileged core. Rust in the shell gets memory safety where the
  most actively-changing, contribution-heavy code lives, without
  inheriting Smithay's unproven BSD backend story (moot here, since
  the shell is just a protocol client, not a backend).
- **Decouples OpenLook-specific work from compositor plumbing.**
  Most contributors will touch olshell. Few need to touch olcore at
  all.

## Shell ↔ Core protocol boundary

olshell talks to olcore *only* via Wayland protocol — no custom RPC,
no shared files, no special sockets beyond what any Wayland client
already uses. Two categories of protocol:

1. **Existing extensions**, reused as-is:
   - `wlr-layer-shell` — for panels, the root menu surface, and the
     workspace switcher strip (anchored, layered surfaces above/below
     normal windows).
   - `xdg-decoration` / custom decoration protocol — so olshell can
     draw OpenLook-style window gadgets (title bar, pushpin, resize
     corners) instead of clients or olcore drawing generic ones.
   - `wlr-foreign-toplevel-management` — lets olshell enumerate and
     manipulate other clients' windows (for the workspace
     switcher, alt-tab equivalent, etc.) without olcore needing
     OpenLook-specific logic baked in.

2. **One small custom protocol extension** (`openlook-workspaces`,
   working name) for the one thing not covered by existing
   extensions: linear workspace switching. Minimal surface —
   something like: `get_workspace_count`, `switch_to(index)`,
   `workspace_changed` event. Kept intentionally tiny so olcore
   doesn't accumulate OpenLook-specific policy — it just tracks
   "which workspace is active" and reports window-to-workspace
   assignment.

**Principle:** olcore should know nothing about OpenLook's *look*.
It's a generic, well-behaved wlroots compositor with one small
workspace extension. All theming, menu behavior, and visual identity
lives in olshell. This keeps the core reviewable by anyone familiar
with wlroots compositors generally, not just this project.

## Repo layout (proposed)

```
/core/     — olcore, C, wlroots-based compositor
/shell/    — olshell, Rust, Wayland client
/protocol/ — shared .xml protocol definitions (openlook-workspaces, etc.)
/docs/     — design docs, contributor guides
```

## Open questions for v0.2

- Window gadget chrome: v1 is implemented -- olcore negotiates
  `xdg-decoration` (server-side) and exposes a new `openlook-decoration`
  protocol (`protocol/openlook-decoration-unstable-v1.xml`) that lets
  olshell attach a header (title bar) surface to any toplevel it can see
  via wlr-foreign-toplevel-management, positioned/stacked by olcore so it
  moves and raises with the window it belongs to. olshell draws a header
  with a window-menu button and centered title for every toplevel, and
  dragging the header (outside the button) moves the window via a `move`
  request on the decoration protocol. Clicking the button now opens the
  window menu too -- a `wl_subsurface` of the header (both are
  olshell-owned surfaces, so this needed no protocol extension, unlike the
  header itself) listing the reference set minus `Refresh` (see
  `docs/OPENLOOK-REFERENCE.md` -- it exists to work around a class of X11
  repaint bug Wayland's damage tracking makes structurally impossible, so
  there's nothing to wire it to; dropped rather than kept as a dead entry).
  `Close`, `Full Size`, `Move`, `Resize`, `Back`, and `Quit` are wired to
  real actions: close/set_maximized via wlr-foreign-toplevel-management,
  `move`/`resize` requests on the decoration protocol mirroring
  xdg_toplevel's own move/resize (each takes a `held` argument since olcore
  can't reliably infer whether the triggering button is still down --
  confirmed live it can't -- so olshell states it explicitly; `resize`
  additionally always passes bottom|right since there's no resize-corner
  chrome yet to pick a different edge from), a `lower` request for `Back`
  (instant, not a grab -- opposite of the raise-on-focus that already
  existed), and a `quit` request for `Quit` -- unlike `Close` (which only
  closes this one toplevel), `quit` sends the same polite xdg_toplevel
  close to every toplevel sharing this one's `wl_client` (the actual live
  connection, i.e. running app instance -- deliberately not matching by
  `app_id`, which two separately-launched instances of the same app would
  share despite needing to stay independent; only olcore can see the real
  grouping). `Stick` is deliberately still a placeholder -- see the
  workspace-switching bullet below, it has nothing to opt out of yet.
  `Properties` (shown disabled) logs a placeholder on click too, same as
  the root menu's non-interactive submenus. No keyboard focus on the
  window menu yet, so only click-elsewhere closes it, not Escape --
  follow-up, same as Escape support was for the root menu. Still no footer
  or resize-corner chrome.
- ~~Root menu behavior/config format~~ resolved: olwm-compatible
  `.openwin-menu`, implemented in `shell/src/menu.rs`.
- Multi-monitor behavior for the workspace strip (per-monitor
  workspaces vs. shared).
- ~~Workspace switching has no visual effect~~ / ~~panel toplevel-title
  list~~ resolved together, as planned: `workspaces_manager_handle_switch_to`
  (`core/main.c`) now actually hides/shows toplevels via a shared
  `update_toplevel_visibility()` helper (a toplevel is visible iff it's
  on the active workspace *and* not minimized -- the two are independent
  reasons to be hidden and must combine rather than clobber each other),
  and refocuses to the most-recently-used visible toplevel on the new
  workspace (or clears focus if none) so keystrokes don't keep going to a
  window that just disappeared. The panel's old toplevel-title list is
  replaced with a workspace switcher strip (`draw_panel` in
  `shell/src/main.rs`): one numbered segment per workspace, active one
  filled with `BACKGROUND_COLOR` as a visual tie to the desktop it
  represents, SELECT-click switches. No reference exists for this
  specifically -- the non-goals above already replace olvwm's VDM pager
  with plain discrete workspaces, so there's nothing authentic to match,
  just a plain segmented strip. `Stick` (the window menu's remaining
  placeholder) can now be implemented for real against this.
- Theming flexibility: later on, users may want to choose between a
  "pure" original-OPEN-LOOK visual style and the more Sun-ified look
  seen in later desktops built on it (see the taskbar/icon convention
  called out as out-of-scope in `docs/OPENLOOK-REFERENCE.md`'s window
  menu section, for one example of the kind of divergence a theme
  might span). Nothing to build now, but worth keeping olshell's
  decoration/panel/menu rendering code factored so style constants
  (colors, spacing, shapes) stay centralized and swappable rather than
  scattered through drawing logic, so a future theme layer doesn't
  need a rewrite to slot in.
