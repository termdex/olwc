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

  ~~No keyboard focus on the window menu yet, so only click-elsewhere
  closes it, not Escape~~ resolved: unlike the root menu, a plain
  subsurface has no wlr-layer-shell Exclusive-interactivity equivalent to
  ask for keyboard focus with, so this needed a small openlook-decoration
  addition -- `grab_keyboard(surface)`/`release_keyboard()` requests on
  the manager, letting olshell hand any of its own surfaces seat keyboard
  focus outright. `open_window_menu` calls `grab_keyboard` once, on the
  window menu's own surface; `close_window_menu` never needs
  `release_keyboard` explicitly, since it always destroys that surface
  right away, and olcore's own destroy-listener counterpart
  (`keyboard_grab_surface_handle_destroy`) hands focus back to the
  most-recently-focused toplevel when that happens -- the same
  `restore_toplevel_focus` helper `layer_surface_destroy` already used
  for the equivalent case on a closing layer surface, factored out for
  the second caller.

  Live testing surfaced one more real gap this exposed: `server_cursor_button`
  calls `focus_toplevel` on every click against a toplevel's own scene
  subtree, which includes any decoration subsurface stacked on it --
  including the window menu itself. Without a guard, clicking a menu item
  that doesn't close the menu (Move to Workspace, which just opens its
  own submenu) would immediately steal focus back to the toplevel's own
  surface, breaking Escape for the still-open menu. Fixed by having
  `focus_toplevel` skip only its final keyboard-reassignment step (not
  the raise/activate/MRU-reorder steps above it, which still make sense
  regardless) whenever a `grab_keyboard` grab is active -- olshell already
  closes the window menu on a click anywhere else, so this never leaves a
  stale grab stranded; it's released within the same round trip either
  way.

  A second round of live testing raised a real UX question rather than a
  bug: with the Move to Workspace submenu open, should Escape close just
  the submenu or the whole window menu? Made it match the mouse
  convention already established for the same row (clicking Move to
  Workspace again closes just the submenu it opened, not the window menu
  behind it) -- one level at a time, closest thing to a precedent this
  codebase already has for it. Since keyboard focus stays on the window
  menu's own surface throughout (the submenu never gets its own grab --
  see WindowMenu's doc comment), press_key can't tell submenu-open from
  window-menu-only-open by *which surface* has focus the way it does for
  everything else; it checks `workspace_submenu.is_some()` instead.

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
- ~~Multi-monitor behavior for the workspace strip (per-monitor
  workspaces vs. shared)~~ resolved: per-monitor, following i3/Sway's
  convention (each output cycles its own independent sequence) rather
  than GNOME/Windows's single desktop-wide sequence -- the strongest
  precedent for a wlroots-based compositor specifically, and it avoids a
  secondary reference monitor getting yanked to a different workspace
  just because the primary switched. `openlook-workspaces-unstable-v1.xml`
  now has a per-output object (`zopenlook_workspaces_output_v1`, obtained
  via a new `get_output_workspaces` request on the manager) carrying
  `switch_to`/`assign_toplevel`/`workspace_count`/`active_changed` --
  previously flat on the manager, now meaningless un-scoped to an output.
  The manager keeps only `get_output_workspaces` and `toplevel_workspace`
  (which now also reports which output an assignment is relative to).
  olcore moved `workspace_count`/`active_workspace` from `olc_server`
  onto `olc_output`, and gave `olc_toplevel` an `output` field tracking
  which monitor it's considered to be on -- set at map time to whichever
  output is under the cursor (`place_new_toplevel`, matching Sway's
  "new windows go on the focused output" convention, since olcore has no
  separate notion of a focused output to track), and updated when an
  interactive move ends with the cursor over a different output
  (`reset_cursor_mode`) -- the scene-graph position is already correct
  regardless, since outputs share one continuous coordinate space, this
  just keeps the bookkeeping in sync with where the window visually
  landed. `assign_toplevel` on the per-output object also reassigns a
  toplevel's output when it targets a different one than the toplevel is
  currently on, which is what makes ADJUST-click on a panel segment a
  real cross-monitor move -- it already called through whichever panel
  was clicked, so this fell out with no changes needed on the olshell
  side of that gesture. olshell creates one panel per output (a new
  `WorkspacePanel`, replacing the flat panel state `Olshell` used to
  carry directly) in `OutputHandler::new_output`, which already existed
  as an empty stub -- a layer surface explicitly bound to that output
  (`create_layer_surface`'s `output` argument, previously always `None`,
  which is why only one monitor ever got a panel at all before this) plus
  its own `zopenlook_workspaces_output_v1`. `OutputHandler::output_destroyed`
  (also a stub before this) tears the panel down again; olcore doesn't
  need to send anything new for that since the ordinary `wl_output`
  global going away is already the standard signal. The window menu's
  Move to Workspace submenu resolves the decorated toplevel's own current
  output (a new field on `ToplevelInfo`, fed by `toplevel_workspace`) to
  find the matching panel. ~~It stays scoped to that one output's
  workspaces -- showing every other monitor's workspaces there too is
  real follow-up work, not built yet.~~ Resolved in a later pass -- see
  the Move to Workspace submenu bullet further down. ~~The root menu's background layer
  surface is unchanged (still one, still `output: None`) -- right-click
  to open it still only works on whichever one output the compositor
  happens to pick; a pre-existing gap this pass didn't cause and didn't
  fix, worth its own pass later.~~ Resolved in a later pass -- see the
  root-menu background bullet below. Verified with `WLR_WL_OUTPUTS=2`
  (wlroots' nested-backend multi-output simulation): two independent
  panels render correctly, each reporting and tracking its own
  workspace_count/active_workspace; interactive switching, cross-monitor
  dragging, and ADJUST-click need real pointer input to verify, handed
  off for live testing the same way other pointer-driven features have
  been this session.

  That live testing surfaced a real bug, not just in the new per-output
  code but in a gap that had been there all along and simply never
  mattered with one output: olcore never called
  `wlr_cursor_map_input_to_output`, so pointer devices were never mapped
  to a specific output. Harmless with a single output (the layout's
  bounding box and that one output's box are the same thing), but
  wlroots' `wayland` backend creates a separate pointer device per
  simulated output (each carrying an `output_name` hint, meant exactly
  for this), and without the mapping, absolute motion events from one
  output's device get normalized against the *entire* multi-output
  layout instead of just that output's own region -- confirmed live as
  a 2:1 sweep ratio and motion on one panel highlighting the other
  entirely, both eliminated once `server_new_pointer` maps each device
  to its matching output by name.
- ~~Root menu background spanning all outputs~~ resolved: the
  `background` layer surface (olshell's OPEN LOOK "root window" -- the
  surface a right-click MENU-click on the desktop opens the root menu
  from) is now created per-output, the same pattern the multi-monitor
  workspaces pass above established for panels. A new `BackgroundOutput`
  struct, mirroring `WorkspacePanel`, holds one background layer surface
  per output; `OutputHandler::new_output` creates it bound to that output
  (`create_layer_surface`'s `output` argument, previously always `None`,
  which is why only one monitor -- whichever the compositor happened to
  pick -- ever got a root-window background, and so a working right-click
  menu, at all), and `output_destroyed` tears it down again, same as a
  panel. `open_menu` now takes the clicked background's own output and
  binds the popup layer surface to it too, so the menu opens on the same
  monitor as the click that triggered it rather than wherever the
  compositor would otherwise place an unbound `Overlay` surface.
  Exiting olshell when its background layer closes -- previously the
  "compositor doesn't want us anymore" signal, back when there was only
  ever one background -- now happens only once the *last* background is
  gone, i.e. once every output has disappeared, rather than on any one
  monitor's background closing. Verified with `WLR_WL_OUTPUTS=2`: both
  nested outputs now render their own background (previously only one
  did, confirming the bug); the resulting per-output root menu placement
  needs real pointer input to confirm, handed off for live testing the
  same way other pointer-driven features have been.

  That live testing (launching a program from WL-2's now-working root
  menu) surfaced another real bug, again pre-existing and simply never
  exposed before: a new window launched from WL-2's menu was correctly
  *tracked* as belonging to WL-2 (its workspace visibility followed
  WL-2's switcher, not WL-1's) but was visually placed inside WL-1's
  screen. `arrange_output_layers` (`core/main.c`) computes
  `output->usable_area` -- the box `place_new_toplevel` positions new
  windows within -- via `wlr_scene_layer_surface_v1_configure`, which
  works in output-local coordinates (0,0 at that output's own top-left),
  the same space `full_area` is deliberately given it in. The layer
  surfaces themselves then get translated into the shared global
  scene-graph space by the `wlr_scene_node_set_position` call right
  below, adding the output's `wlr_output_layout_get_box` offset -- but
  `usable_area` was stored straight from the local-space calculation,
  missing that same translation. Every output's usable area therefore
  came out identical to whichever output happens to sit at (0,0) in the
  layout, so `place_new_toplevel` placed every new window inside that
  one output's box regardless of which output it was actually meant
  for -- harmless-looking with one output (there's only the one box to
  land in), and never exercised for a non-primary output until this
  pass made a second output's root menu actually usable. Fixed by adding
  the same `output_box.x`/`output_box.y` translation to `usable_area`
  before storing it.

  A second round of live testing (pinning the root menu on one output,
  then right-clicking the other) surfaced a real design gap rather than
  a bug: `Olshell` had only ever had a single `popup: Option<MenuPopup>`
  slot, so `open_menu` unconditionally replaced whatever popup existed
  -- harmless when there was only ever one output to open a menu on, but
  now meant opening a menu on WL-2 silently undid a pin made on WL-1.
  Fixed by making `popups: Vec<MenuPopup>` and having `open_menu` drop
  only *unpinned* popups before pushing a new one: OPEN LOOK only ever
  shows one transient menu at a time, so that part of the old behavior
  is preserved, but a pinned popup is now a true persistent palette,
  independent of whatever else olshell does -- any number can be open at
  once, one or more per output. Every place that used to reach for "the"
  popup (item/pushpin hit-testing, layer-shell `configure`/`closed`,
  Escape) now looks it up by surface via a new `popup_at()` helper (same
  family as `panel_at`/`background_at`), except Escape, which can't use
  the pointer's surface -- added a `keyboard_focus: Option<WlSurface>`
  field, kept current by `KeyboardHandler::enter`/`leave` (both no-ops
  before this), so Escape closes whichever popup olcore actually handed
  keyboard focus to rather than assuming there's only one candidate.
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
- ~~Move to Workspace submenu is single-output only~~ resolved: it now
  lists every output's workspaces, not just the decorated toplevel's own
  current one -- the current output's first (keeping the plain flat list
  a single-monitor setup always had, unchanged), then every other
  output's, each introduced by its own non-interactive header row naming
  that output (`OutputState::info`'s `name`, the same identifier
  `WLR_WL_OUTPUTS` gives the nested-backend outputs used to test this).
  Header rows only appear once there's more than one output at all, so a
  single-monitor setup grows no new chrome it has no use for. Modeled as
  `WorkspaceSubmenu::rows: Vec<WorkspaceSubmenuRow>` (an `OutputHeader` or
  a `Workspace { output, index, current }`) rather than nesting another
  level of submenu per output -- keeps every target a single click away,
  and the row list is short enough in practice (a handful of workspaces
  times a handful of monitors) that flattening it doesn't get unwieldy.
  Clicking a row on a *different* output than the toplevel's own is what
  this submenu was actually missing mechanically, not just missing rows
  to click -- `assign_toplevel` on the per-output workspaces object has
  reassigned a toplevel's `output`/`workspace_index` bookkeeping since
  the multi-monitor pass above (the same request ADJUST-click on a panel
  segment already used), but live-testing this submenu's new
  cross-monitor rows caught that `output_workspaces_handle_assign_toplevel`
  (`core/main.c`) never actually repositioned the toplevel's scene node
  when the target output differs from its current one -- it stayed
  rendered wherever it physically was on the *old* output's screen
  (a different box in the same shared global coordinate space) while its
  bookkeeping correctly followed the new one, which was visible as the
  new output's workspace switcher controlling a window that still looked
  like it belonged to the old one. This was already latent before this
  pass -- ADJUST-click has the identical gap, just apparently never
  exercised across two real outputs before now -- as opposed to a
  cross-monitor *drag* (`reset_cursor_mode`), which never needed this:
  the drag itself already leaves the scene node wherever the cursor
  dropped it. Fixed by repositioning to the target output's
  `usable_area` top-left, the same place `place_new_toplevel` puts a
  brand new window, whenever the output actually changes.

  That fix immediately surfaced a second, related bug on the very next
  round of live testing: a *decorated* window moved this way landed with
  its header behind the panel instead of just below it. A header
  decoration pushes its toplevel's own scene node down by the header's
  height, once, at the moment `get_decoration` attaches it (see that
  request handler's "push the toplevel itself down" comment) -- the
  header then sits *above* that pushed-down position, at the toplevel's
  original spot. `place_new_toplevel` never needs to account for this,
  since a toplevel is always undecorated at map time (olshell only
  requests a decoration afterward, once it learns about the new toplevel
  via wlr-foreign-toplevel-management) -- but a toplevel this
  reassignment moves is very often already decorated, so repositioning
  it straight to `usable_area`'s top-left put its *content* where the
  *header* belongs, pushing the header itself above `usable_area` and
  behind the panel. Fixed by adding the decoration's height to the
  target y-coordinate when the toplevel has one.
- Icon tray / minimize gap: investigating a question about the icon row
  along the bottom of the reference screenshots (see
  `screenshots/sunos551-ow1-scr-01/02/03.png`, which catch Calendar
  Manager and a Text Editor window transitioning between open-window and
  icon form across the same session) confirmed those are iconified
  (minimized) windows sitting loose on the desktop, not app-launcher
  shortcuts -- launching is the root menu's job, already implemented.
  OPEN LOOK's own vocabulary splits what most window managers call
  "close" into two: `Close` iconifies (the app keeps running), `Quit`
  actually terminates it. olwc's window menu already has both labels,
  but `Close` is currently wired to a real `wlr_foreign_toplevel_handle_v1`
  close request, not iconify -- a mismatch with authentic OPEN LOOK
  semantics. olcore separately already has full minimize-state tracking
  (`olc_toplevel::minimized`, wired to wlr-foreign-toplevel-management's
  own `request_minimize` for clients that ask to minimize themselves) and
  visibility logic honoring it, but nothing in the window menu drives it,
  and there's no olshell UI at all to show or restore a minimized window
  once it's gone -- the desktop-icon tray itself isn't built. Worth its
  own design pass: whether `Close` should be repointed at minimize (with
  `Quit` becoming the only real close/terminate path), and what an icon
  tray looks like in olshell's plainer, non-VDM visual language.
