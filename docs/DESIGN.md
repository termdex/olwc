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
  `Close`, `Full Size`, `Move`, `Resize`, `Back`, `Quit`, and `Stick` are
  wired to real actions (`Stick` described separately below);
  close/set_maximized via wlr-foreign-toplevel-management,
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
  grouping). `Properties` (shown disabled) logs a placeholder on click
  too, same as the root menu's non-interactive submenus.

  `Stick` (a `toggle_sticky` request) exempts a toplevel from the
  per-workspace hiding the workspace switcher strip below causes.
  Un-sticking commits the toplevel to whichever workspace is active at
  that moment, rather than reverting to wherever it was before it was
  stuck -- confirmed live that snapping back is surprising, since the
  window can appear to vanish if you've switched workspaces since. Unlike
  the other toggle (`Full Size`, whose maximized state comes for free
  from wlr-foreign-toplevel-management), olcore has to explicitly report
  sticky state back via a `sticky_changed` event, since two things need
  to know it: the window menu shows `Unstick` instead of `Stick` once
  toggled on, and the header draws a small pushpin while sticky (reusing
  the same glyph the root menu's pin-to-persist gesture uses, since both
  mean "stays put").

  No keyboard focus on the window menu yet, so only click-elsewhere
  closes it, not Escape --
  follow-up, same as Escape support was for the root menu.

  Resize chrome is now complete: all four corners plus a footer strip
  between the two bottom ones, unified under one `ResizeRegion` enum
  (`shell/src/main.rs`) rather than bolted on separately, since the
  reference groups the footer with the corners, not the header ("a bottom
  strip dedicated to resize, separate from the header"). All five are
  subsurfaces of the header, same trick as the window menu; bottom
  corners and the footer are positioned at the toplevel's actual bottom
  edge via a `toplevel_height` argument on the `configure` event --
  olshell has no other way to learn this, since
  wlr-foreign-toplevel-management deliberately carries no geometry. Top
  corners needed the header's own button and sticky indicator shifted
  inward to make room, rather than overlapping them. Dragging any of the
  five sends `resize` with `held: 1` (a real press-hold-drag gesture,
  unlike the window menu's Resize item, which is a discrete click and
  needs `held: 0`) and the region's corresponding edge bitmask. The
  corners are drawn as filled circles (an explicit placeholder for OPEN
  LOOK's true obround shape -- not visible at usable resolution in either
  reference screenshot) and the footer as a thin bar, both on otherwise
  fully transparent buffers so they float over the toplevel's own content
  rather than sitting in a visible box; getting that transparency to
  actually render correctly needed two separate fixes found live -- each
  handle's first real content needs a fresh parent commit after the
  subsurface relationship is established (same gotcha the window menu
  hit, just timed differently: the early "nudge" commit in
  `ensure_decoration` fires before any of them have content yet, so it
  doesn't count on its own), and "transparent" pixels need their RGB
  zeroed along with alpha -- wl_shm Argb8888 buffers are expected
  premultiplied, and stale RGB (SlotPool buffers are reused memory) with
  zero alpha isn't validly premultiplied, so it rendered as a faint ghost
  of whatever was drawn in that memory before instead of true
  transparency.

  Focus is now indicated on the header itself, matching the reference
  screenshots: the focused window's header fills with a darker gray
  (`DECORATION_FOCUSED_BG_COLOR`) and its bevel flips from raised
  (light top edge, dark bottom edge -- the unfocused, "unpressed"
  look) to inset (dark top, light bottom), reusing the same
  raised/inset bevel language `OPENLOOK-REFERENCE.md` already
  describes for buttons, applied here to focus rather than a press.
  Driven by state code 2 ("activated") on
  wlr-foreign-toplevel-management, which `draw_decoration` already
  had access to; no new protocol needed. The button's unhovered fill
  now tracks the header's own background color instead of always
  being the unfocused shade, so it doesn't look mismatched against a
  focused header.

  The resize-corner glyphs are no longer filled circles. Cross-checking
  `screenshots/sunos551-ow1-scr-01.png`'s Text Editor window at all
  four corners (pixel-sampled, not just eyeballed) showed OPEN LOOK's
  actual resize-corner shape is a right-angle bracket hugging the
  corner -- a "framing square," not the obround/pill shape assumed
  earlier (that guess predated having a screenshot where the corners
  were legible at all). `draw_corner_handle` now draws that: each
  corner's bracket elbow sits at the corner itself, with its two arms
  reaching along the two edges away from it, via a generic
  flip-and-scan routine driven by `ResizeRegion::corner_flip()` rather
  than four hand-written cases. Bevel direction is the same absolute
  top-left-light/bottom-right-dark convention used everywhere else in
  the chrome (not one that rotates with the bracket) -- confirmed
  against the same screenshot, where the top-right corner's bracket
  still has its top- and left-facing surfaces lit and its right-facing
  surface shadowed, same as bottom-right's.

  Every window now also has a plain black border, 3px thick, running
  around the whole frame -- header top and sides, then continuing down
  the toplevel's own left/right edges and across its bottom -- confirmed
  against the same screenshot (pixel-measured, not estimated: exactly
  3px on all four sides). It's a separate visual element from the
  header's own focus bevel, not a replacement for it: the border marks
  the window's actual edge and sits outermost, with the existing
  light/dark bevel row drawn just inside it. The border stops exactly
  at each corner bracket's own footprint rather than running underneath
  it, matching the reference -- a corner bracket's fill is only partly
  opaque (the notch is transparent, revealing the header underneath), so
  anything drawn under it has to stop at its edges or it bleeds through.
  Implementation: the header's own top stretch is drawn directly into
  its existing buffer (`draw_decoration`); the two side stretches are a
  new `BorderStrip` subsurface pair (like the resize handles, but
  non-interactive -- no hover, not part of `ResizeRegion`) spanning from
  below the top corner to above the bottom one on each side, since they
  have to cover both the header and the toplevel's content below it;
  the bottom stretch is one more `fill_rect` inside the existing footer
  buffer.

  The window menu now has its first real submenu: `Move to Workspace`,
  deferred earlier until submenu support existed (see the
  workspace-switcher entry below). Clicking it opens a `WorkspaceSubmenu`
  -- a subsurface of the window menu's own surface, positioned to its
  right and top-aligned with that row, listing every workspace directly
  (`Workspace 1`, `Workspace 2`, ...) rather than nesting further.
  Clicking a workspace sends `assign_toplevel`, the same request
  ADJUST-click on the panel already uses, and closes both menus; the
  toplevel's own current workspace is shown disabled in the list (same
  convention as `Properties`), since moving a window to where it already
  is is a no-op -- tracked via a new `workspace_index` field on
  `ToplevelInfo`, kept in sync by the workspaces protocol's
  `toplevel_workspace` event, which olshell received but discarded
  before this. Clicking `Move to Workspace` again while its submenu is
  open closes just the submenu, the same toggle the header button
  already uses for the window menu itself; clicking any other window-menu
  item closes both, same as it always closed one. A small rightward
  wedge (`draw_submenu_arrow`, mirroring `draw_chevron`) marks the row as
  opening a submenu rather than acting immediately -- the only item with
  one so far, but every row's width now reserves space for it, so a
  future submenu item doesn't need a different-width popup.

  Live testing surfaced a real gap: while a window is sticky, the
  submenu's grayed-out "current" row reflected whatever workspace it was
  on *before* being stuck, not where it actually is now (sticky windows
  don't update `workspace_index` -- see the Stick entry above for why),
  which read as wrong. But the deeper issue wasn't the display, it was
  that the whole action is inert for a sticky window: `assign_toplevel`
  would set `workspace_index`, but un-sticking always commits to
  whichever workspace is active *at that moment* regardless of
  `workspace_index` (the same fix that made un-sticking stop making
  windows "disappear"), so nothing the submenu could do would have any
  effect, now or later. Fixed by disabling the `Move to Workspace` item
  itself while sticky, same convention as `Properties`, rather than
  letting a submenu open with nothing meaningful in it.
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
  just a plain segmented strip. ADJUST-click (middle button) on a segment
  moves the currently-focused toplevel to that workspace instead of
  switching to it, via a new `assign_toplevel` request on the workspaces
  protocol -- borrowed from modern multi-workspace desktops rather than
  an OPEN LOOK convention, but a fitting real use for the ADJUST button.
  `Stick` (see the window menu bullet above) is built against this too --
  every window-menu item is now either a real action or an intentional
  placeholder (`Properties`, disabled to match the reference screenshot).
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
