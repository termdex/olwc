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
- OPEN LOOK-styled scrollbars (or any other widget that lives *inside*
  a window's own content area). Scrollbars were a genuine OPEN LOOK
  toolkit widget (XView's `lib/libxview/scrollbar/`, matching this
  doc's own widget vocabulary table's "Scrolled window" row), but drawn
  by whatever *application* linked against the toolkit and used that
  widget -- never by `olwm`/`olvwm` itself, which only ever draws
  chrome from outside a window (title bar, pushpin, resize handles),
  the same boundary olshell draws for itself. A Wayland client owns and
  draws its own content pixels; nothing outside it can reach in to add
  a scrollbar, the same restriction that forced `openlook-decoration`
  to exist just for *external* header chrome. Real OPEN LOOK/olvwm
  screenshots showing scrollbars and others not is just which
  applications happen to appear in each, not a spec inconsistency to
  chase. Styling scrollbars to match would be a concern for some future
  Wayland-native app's own toolkit, entirely outside olcore/olshell's
  scope.

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

  ~~Focus is now indicated on the header itself, matching the reference
  screenshots: the focused window's header fills with a darker gray
  (`DECORATION_FOCUSED_BG_COLOR`) and its bevel flips from raised
  (light top edge, dark bottom edge -- the unfocused, "unpressed"
  look) to inset (dark top, light bottom)~~ corrected in a later pass:
  a live closer look at the reference screenshots -- and, more
  precisely, sampling actual pixel values straight through a real
  focused and unfocused title bar rather than just eyeballing a
  screenshot crop, which turned out to be too JPEG/PNG-compressed to
  show the structure at all -- found that a full-header color swap was
  never the real effect. `screenshots/sunos551-ow1-scr-03.png`'s
  focused Calculator window and unfocused File Manager window, scanned
  column by column: both have the *exact same* light
  `DECORATION_BG_COLOR` at their own top/bottom margins; the focused
  one additionally sinks a smaller, darker panel into that margin via a
  dark-top/light-bottom bevel pair (3px light margin, 1px dark bevel,
  13px darker fill, 1px light bevel, 5px light margin, at that
  screenshot's own resolution), while the unfocused one has no such
  panel, just the thin raised ridge near the bottom this project
  already had. `DECORATION_HEIGHT` grew from 22 to 28 to fit that
  structure comfortably around the existing button/title content
  rather than cramming it into a flat, uniformly-colored bar the way
  the too-shallow previous version did (`DECORATION_FOCUS_MARGIN_TOP`/
  `_BOTTOM`, `shell/src/main.rs`, hold the measured margin sizes).
  Reuses the same raised/inset bevel language `OPENLOOK-REFERENCE.md`
  already describes for buttons -- what changed is recognizing that
  language frames a *sub-region* here, not the header's own full
  extent. Driven by the same state code 2 ("activated") on
  wlr-foreign-toplevel-management as before; no protocol change
  needed. The button's unhovered fill tracks whatever color actually
  sits behind it now (the recessed panel's own fill when focused, the
  header's plain fill otherwise), not a single "the header's
  background" value the way it could when that was one color for the
  whole header. Verified by rendering both states directly through the
  real drawing code (the same self-verification technique the earlier
  pill-centering and Notice work used) rather than through a live
  screenshot, which the floating workspace palette and a maximized
  test window both ended up overlapping during an attempt to capture
  one.

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
  arrow (`draw_submenu_arrow`; asset-accurate since a later pass -- see
  the OLGlyph entry below) marks the row as opening a submenu rather
  than acting immediately -- the only item with one so far, but every
  row's width now reserves space for it, so a future submenu item
  doesn't need a different-width popup.

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

  ~~Window-menu button glyph and pushpin icon shape were placeholder
  geometry~~ resolved, and not by finding a better screenshot: both
  olwm/olvwm and the XView/OLIT toolkits drew this chrome by rendering
  characters from a private bitmap font, OLGlyph, via the shared
  `libolgx` drawing library, rather than drawing bitmaps directly --
  and that font is preserved, pixel-for-pixel, in the historical
  XView/olwm source trees at github.com/MagnetarRocket/xview-openlook
  and github.com/ggodd/xview-64bit (`xview-base/fonts/bdf/misc/
  olgl14.bdf`), still under Sun's original 1989 permissive license.
  `winbutton.c` confirmed the window-menu button is olwm's "abbreviated
  menu button" widget (`olgx_draw_abbrev_button`); `ol_misc.c` confirmed
  the pushpin is `olgx_draw_pushpin`. Traced the relevant glyphs
  (encodings 22/23 for the button, 19/20 for the pushpin -- see below
  for why not 100-105) directly from the BDF source into
  `BUTTON_GLYPH_NORMAL`/`_PRESSED` and `PUSHPIN_GLYPH_PINNED`/
  `_UNPINNED` (`shell/src/main.rs`), replacing `draw_chevron`'s
  geometric wedge and `draw_pushpin`'s filled/outline circle. This is
  reference material the same way the screenshots already were, not a
  code port -- no XView/olwm code was adapted, only the bitmap shapes
  it draws from a font neither toolkit's C source actually contains.

  Getting from "found the bitmap" to "looks right" took three more
  rounds, each a genuine bug rather than a fidelity nice-to-have:

  1. `olgx_draw_pushpin`/`olgx_draw_abbrev_button` actually have *two*
     distinct designs per state, not one -- a three-layer bevel
     composite (separate highlight/fill/shadow glyphs in three colors,
     encodings 100-105 for the pushpin) for 3D rendering, and a
     purpose-built flat single-color glyph (encodings 19/20) for 2D
     rendering. The first attempt traced the 3D composite and flattened
     it to one color, which confirmed live as a compressed-looking
     blob -- a naive union of three offset bevel outlines is thicker
     and blockier than any one of them alone. The button glyph
     (encodings 22/23) happened to already be the flat variant (olwm's
     2D and 3D code paths both use it, unlike the pushpin), which is
     why it rendered cleanly on the first try with no complaint. Fixed
     by switching the pushpin to its own flat variant instead.
  2. `draw_glyph_bitmap`'s first scaling pass point-sampled a single
     nearest source pixel per destination pixel -- ordinary nearest-
     neighbor. Shrinking the unpinned pushpin's 29px-wide source into a
     10px box dropped nearly all of its 1px-wide outline strokes into
     the gaps between sampled points, confirmed live as a handful of
     barely-visible scattered dots. Fixed by having each destination
     pixel turn on if *any* source pixel in the region it covers is on,
     rather than sampling one point in that region.
  3. Coverage sampling fixed the missing-pixels problem but not a
     second one live testing then surfaced: independently stretching
     each axis to exactly fill the box distorts aspect ratio for any
     glyph whose proportions don't match the box's -- confirmed live as
     the unpinned glyph (native ~2:1 wide) still looking noticeably
     horizontally compressed, even fully covered. This mattered here
     specifically because one box (the root menu's `pushpin_rect`) has
     to hold two states with very different native shapes: the pinned
     glyph is a compact near-square, the unpinned one nearly 2:1 wide.
     Fixed two ways together: `draw_glyph_bitmap` now scales uniformly
     (the same factor on both axes) to the largest size that fits the
     box, centered, rather than stretching each axis independently; and
     the popup's pushpin box grew from a 10x10 square
     (`POPUP_PUSHPIN_WIDTH`/`_HEIGHT`, now 26x14) closer to the
     unpinned glyph's own native footprint, so neither state needs much
     shrinking at all. The decoration's sticky indicator only ever
     shows the pinned state, so it kept a square box
     (`STICKY_PUSHPIN_SIZE`), sized to the pinned glyph's native 15x15
     for an exact, unscaled render.

  Surfaced, not fixed, since it's a materially bigger and separate gap:
  none of this accounts for output scale. `CompositorHandler::
  scale_factor_changed` is currently a no-op stub, so every surface
  olshell draws (these glyphs included) renders at 1x regardless of the
  real output scale -- see the HiDPI/output-scale entry above for why
  `draw_glyph_bitmap`'s uniform, coverage-sampled scaling is still the
  right foundation for that whenever it gets built, just parameterized
  by the output's real scale instead of always assuming 1x.

  ~~The root menu's title was drawn in the same weight as its items~~
  resolved: confirmed genuine, not a one-off screenshot artifact, by
  checking XView's own menu widget source
  (`lib/libxview/menu/omi.c`): `if (im->title) font = std_image->
  bold_font; else font = INHERIT_VALUE(font);` -- any menu built with
  XView's menu package gets a bold title and plain-weight items
  automatically, a toolkit-level convention. The bundled font (VT323)
  has no real bold weight -- deliberately a single-weight retro
  terminal typeface -- so a second, stylistically mismatched font
  family wasn't worth bundling just for one line of text; added
  `draw_bold_text_row_centered` instead, a faux-bold renderer that
  draws the text twice, the second copy shifted 1px right, thickening
  strokes via double alpha-blending. Only `MenuPopup`'s own title (e.g.
  "Workspace") uses it -- the window menu has no equivalent title row,
  and the Move to Workspace submenu's per-output header rows are an
  olshell-only grouping label with no XView precedent either way, so
  they're left as plain weight rather than guessing.

  ~~The submenu-arrow indicator (Move to Workspace's row) was a
  geometric wedge~~ resolved: found in `libolgx` after all. It's
  `olgx`'s "menu mark" glyph (`olgx_draw_menu_mark`), the same
  primitive the window-menu button's own arrow uses in 3D mode -- just
  the horizontal orientation (encodings 48-50, `HORIZ_MENU_MARK_UL`/
  `_LR`/`_FILL`) rather than vertical (45-47). `om_render.c` confirmed
  it's exactly XView's own pullright-submenu indicator: `if
  (mi->pullright) olgx_state |= OLGX_HORIZ_MENU_MARK;`. Unlike the
  pushpin, `olgx_draw_menu_mark`'s 2D rendering path draws all three
  layers -- the two outline layers together in one color, then the
  fill layer on top -- so tracing all three combined (rather than
  picking one variant over another, the pushpin's problem) was correct
  from the start: a solid filled triangle, not just an outline. Traced
  into `SUBMENU_ARROW_GLYPH` (`shell/src/main.rs`), rendered through
  the same `draw_glyph_bitmap` the button and pushpin use -- no new
  scaling issues, since the box (`SUBMENU_ARROW_SIZE`, 8x8) and the
  glyph's native size (11x11, already square) are close enough that
  neither the coverage-sampling nor the aspect-preserving fixes from
  the pushpin's own pass had anything to correct here.
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
- Root menu "Exit..." item: authentic, not a modern addition -- confirmed
  from source that olwm's actual default root menu
  (`clients/olwm/openwin-menu` in the historical XView/olwm tree, see
  `docs/OPENLOOK-REFERENCE.md`) is exactly a `Programs` submenu and
  `"Exit..."`, an `EXIT` directive. olshell's `.openwin-menu` parser
  (`shell/src/menu.rs`) now recognizes it (`MenuNode::Exit`), and the
  built-in default menu includes it alongside Terminal/Refresh. In X11,
  Exit terminated olwm itself, which (as the session's leader) normally
  returned control to a display manager; the Wayland-native equivalent is
  terminating olcore's own `wl_display` -- already the dev-only
  `Alt+Escape` binding's whole job, just with no way for an unprivileged
  client to reach it before now. Added a new, deliberately minimal
  protocol for exactly this, `openlook-session-unstable-v1`
  (`zopenlook_session_manager_v1`, a single `exit` request) -- scoped to
  grow into other whole-session actions later (screen lock, suspend, ...)
  as their own requests, the same one-request-per-concept shape
  `openlook-decoration-unstable-v1` already uses for `lower`/`quit`/
  `toggle_sticky`, rather than one generic "session action" request with
  a string/enum argument.

  ~~Deliberately not built alongside this: a confirmation Notice before
  actually exiting.~~ resolved: the `...` in "Exit..." is itself OPEN
  LOOK's own convention for "this shows a confirmation" (see the widget
  vocabulary table's `Notice` entry -- "used specifically for short,
  must-acknowledge messages"), and confirmed from source
  (`clients/olwm/notice.c`, `services.c`'s `ExitFunc`) that real olwm's
  own Exit confirmation shows exactly the message "Please confirm exit
  from window system" with `Exit`/`Cancel` buttons, `Cancel` the safe
  default -- used verbatim. `open_notice`/`draw_notice`
  (`shell/src/main.rs`) build a real `Notice` (a genuine wlr-layer-shell
  top-level surface, not a subsurface -- unlike the window/icon menus, a
  Notice isn't tied to any decoration or icon to hang off of; deliberately
  given no anchor at all, which is what centers it on the output for
  free, and `Exclusive` keyboard interactivity for focus). Fully modal:
  `pointer_frame` swallows every pointer event on any other surface while
  one's open (mirroring the armed-icon-drag swallow-check's own
  precedent), and Escape/Return both dismiss without acting, matching
  `NOTICE_DEFAULT_BUTTON` being `Cancel`.

  The Notice's buttons turned out to be a natural second use for the
  pill-tiling machinery the menu-item highlight entry above already
  built: real OPEN LOOK's standalone "oblong button" and its menu-item
  highlight are the same `olgx_draw_button`/`olgx_draw_accel_button`
  composite, just different colors and, for a real button, a centered
  label instead of one the caller draws separately. `draw_pill_highlight`
  is now a thin wrapper around a colors-parameterized `draw_pill`, and a
  new `draw_button` draws a real clickable button on top of it (raised,
  light-top/dark-bottom, unless pressed, matching the same raised-unless-
  invoked convention used throughout olshell's chrome) -- confirmed by
  rendering the whole Notice directly through the actual drawing
  primitives before wiring up live interaction, the same self-
  verification technique the pill-centering bug used.

  One deliberate simplification, not full fidelity: real olwm's frame is
  a nested "chiseled" double `olgx_draw_box` (a recessed outer box around
  a raised inner one); the Notice here uses a single bevel layer instead
  -- distinct enough to read as its own "boxed" element (no existing
  olshell popup has a frame at all), without needing a second widget-
  drawing technique built just for one dialog's border.

  Raised the broader question of Sleep/Shutdown/Restart/Session actions
  (KDE-style, not an OPEN LOOK convention at all) -- deliberately kept
  separate from this entry rather than folded in, since each is a real,
  standalone feature: Suspend/Shutdown/Restart need `logind` D-Bus
  integration (`org.freedesktop.login1`'s `Suspend`/`PowerOff`/`Reboot`),
  and a lock screen (for Session) needs `ext-session-lock-v1`, a protocol
  olcore doesn't implement at all yet -- switching users is a bigger
  scope still (VT switching plus spawning another greeter), likely out of
  scope for a project this size rather than a near-term to-do.
- ~~Workspace switcher as a persistent full-width edge bar~~ resolved: the
  most visibly non-authentic piece of chrome left in olwc, on reflection
  -- a permanent screen-edge strip reserving space (`set_exclusive_zone`)
  is a GNOME/Windows-taskbar-era convention, not an OPEN LOOK one at all.
  `WorkspacePanel` is now a small floating palette instead: sized to its
  own content (`workspace_count` segments) rather than the output's
  width, anchored to just the top-left corner rather than spanning
  `TOP|LEFT|RIGHT`, and carrying no exclusive zone at all -- windows get
  the full output underneath it now. The closer authentic precedent,
  confirmed from source (`clients/olvwm-4.1/olvwm.man`): olvwm's own VDM
  (Virtual Desktop Manager) was a real, freely positionable window
  (`VirtualGeometry`, default `+0+0`, used as this palette's own default
  position) and even iconifiable (`VirtualIconGeometry`) -- a spatial
  panner over a 2D virtual desktop (`VirtualDesktop` defaults to `3x2`
  screens), not a linear workspace list, so the *content* doesn't map
  over (olwc deliberately kept its own linear, per-output workspace
  model, established when multi-monitor support was built above), but
  the *presentation* -- a small, movable palette, not edge-docked chrome
  -- does.

  Draggable: SELECT-press-and-drag anywhere on it repositions the whole
  palette, reusing the exact same click-vs-drag ambiguous-until-
  threshold pattern already established for icon dragging (`PanelDrag`,
  `shell/src/main.rs`), clamped to stay fully on the output. A plain
  click (released before crossing the threshold) still switches
  workspace, unchanged; ADJUST-click (move the focused window to a
  segment) is untouched either way. One real subtlety a moving *surface*
  creates that a moving *icon* (drawn within an unmoving background
  surface) never had to deal with: each motion event's own coordinates
  are reported local to wherever the surface currently is, which keeps
  changing mid-drag as this very gesture repositions it -- comparing
  local coordinates directly across events, the way icon dragging safely
  can, would be wrong here. Fixed by converting each event's local
  position to output-absolute coordinates *using the panel's position as
  of that specific event* (always correct, since that's what the
  compositor actually used to compute it) before doing the same origin-
  plus-total-delta math icon dragging already does.

  Confirmed live: a screenshot shows the palette rendering as a compact
  box near the corner rather than the old full-width strip, with the
  background filling the whole output beneath it. Dragging itself needs
  a live hand to test (no synthetic pointer input available), so that
  part is unverified beyond the code and math above.

  Deliberately not built alongside this: any visual frame/border to read
  more distinctly as "a little window" the way the Exit confirmation
  Notice's own beveled frame does -- right now it's still just a bare
  cluster of squares, floating. Worth a follow-up if the bare-squares
  look doesn't feel enough like a window once seen dragged around live.
- Settings GUI tool: several things olwc already supports or has flagged
  as a real decision have no user-facing way to change them at all today
  -- worth a proper Settings app once enough of them exist to justify one,
  the way Plasma/GNOME each have. Candidates already identified: output
  scale (the HiDPI entry below only has a startup-only debug env var,
  `OLC_TEST_OUTPUT_SCALE` -- there's no protocol-level way to change a
  real monitor's scale/resolution/position at runtime at all yet, dev
  knob or not; that needs olcore to implement `wlr-output-management-
  unstable-v1`, the standard wlroots protocol for exactly this, before a
  Displays pane could do anything), workspace count (`OLC_WORKSPACE_COUNT`
  in `core/main.c` is a compile-time `#define`, not configurable), cursor
  theme/size (`wlr_xcursor_manager_create(NULL, 24)` in `core/main.c`
  hardcodes both), the ADJUST-button mapping question (three-button-as-is
  vs. modifier+click for modern hardware, still open in
  `docs/OPENLOOK-REFERENCE.md`), theming (see the entry above), and
  window-menu keyboard accelerators (see that entry below) once built.
  Root-menu contents (`.openwin-menu`) deliberately left off this list --
  that's a config file by design, same as i3/sway, not obviously a GUI
  settings item.

  Toolkit choice for building it: lean toward growing olshell's own
  drawing primitives and widget vocabulary (the OLGlyph button/pushpin/
  arrow tracings, obround/beveled chrome, menu/popup click-and-hover
  handling) into a small shared crate, rather than adopting GTK or Qt --
  neither looks like OPEN LOOK out of the box (Adwaita or a modern QStyle
  would need heavy re-theming to get obround buttons and top-left-light/
  bottom-right-dark bevels back), and both pull a large dependency graph
  into a codebase that's been deliberately lean everywhere else (fontdue
  instead of a full text-shaping stack, hand-rolled pixel drawing instead
  of Cairo/Skia). olshell has already organically grown a good chunk of a
  minimal toolkit to get its own chrome right; finishing that internally
  -- an informal "libolgx-in-Rust" underneath both olshell and a future
  Settings app, the same relationship real OPEN LOOK's `libolgx` had to
  every application built on it -- is less total work than re-fighting a
  borrowed one, and keeps every future app visually consistent with the
  compositor's own look for free.
- ~~HiDPI / output scale support: `CompositorHandler::scale_factor_changed`
  (`shell/src/main.rs`) is currently a no-op stub -- every surface olshell
  draws renders at 1x regardless of the output's real scale.~~ resolved:
  integer `buffer_scale` support, built as anticipated below -- fractional
  (150%, etc., via `wp_fractional_scale_v1`) stays a distinct, explicitly
  deferred follow-up, not attempted here, per Wayland's own two-layer scale
  model (see the original reasoning, kept below).

  Every surface-owning struct that matters (`BackgroundOutput`,
  `WorkspacePanel`, `MenuPopup`, `WindowMenu`, `IconMenu`, `Decoration`)
  gained a `scale: i32` field, updated by the now-real
  `scale_factor_changed`, which dispatches by surface identity using the
  same helpers `pointer_frame` already relies on for this
  (`background_at`, `panel_at`, `decoration_toplevel_id`, `popup_at`,
  direct `==` for `window_menu`/`icon_menu`). Deliberately *not* added to
  `WorkspaceSubmenu` or `Decoration`'s seven child chrome pieces (footer,
  4 `ResizeHandle` corners, 2 `BorderStrip`s) -- each of those always
  redraws through its owning struct's own draw call (`draw_window_menu`
  redrawing an open submenu too; `draw_decoration` already redraws all
  seven pieces internally every call), reading that one owner's `scale`,
  since a submenu or corner glyph is always positioned within its parent's
  bounds and so is on the same output/scale in every realistic case.

  The actual scaling work turned out to be entirely confined to a handful
  of shared low-level pixel-writing primitives (`fill_rect`,
  `draw_text_row_centered`/`draw_bold_text_row_centered`,
  `draw_glyph_bitmap`, `paint_row`, plus the free-standing
  `draw_corner_handle`/`draw_footer`/`draw_border_strip`), each of which
  now takes a `scale: i32` and multiplies its own logical coordinate
  arguments internally before touching the canvas -- every `draw_*`
  function's own layout arithmetic (which rect goes where, hit-testing,
  `font.metrics()` centering-width lookups, every `subsurface.
  set_position()` call) stays completely untouched, since Wayland already
  keeps all of that logical-space state independent of `buffer_scale`; only
  the final raster step -- the buffer's physical pixel size and what
  `set_buffer_scale()` is told -- changes. One genuine landmine surfaced by
  this split: several `draw_*` functions reused the same `width`/`height`
  variable both for logical coordinate math (e.g. `width - CORNER_HANDLE_SIZE`)
  and as the primitives' physical `canvas_width`/`canvas_height` bound --
  fixed by introducing a separate `buf_width`/`buf_height` pair (`=
  logical * scale`) for buffer creation and the primitives' canvas-size
  arguments, leaving the original logical `width`/`height` untouched for
  everything else.

  No physical HiDPI display exists in this dev environment, so
  verification needed a way to simulate one: `server_new_output`
  (`core/main.c`) gained a debug/testing-only env var,
  `OLC_TEST_OUTPUT_SCALE`, calling `wlr_output_state_set_scale()` (a real
  wlroots API the compositor never previously called) -- no protocol or UI
  surface of its own, defaults to wlroots' own 1.0 when unset, same spirit
  as the wlroots-provided `WLR_NO_HARDWARE_CURSORS` knob already relied on
  for testing all session. Confirmed live at `OLC_TEST_OUTPUT_SCALE=2`:
  the panel, decoration header (title text and the OLGlyph-traced button
  glyph and resize-corner bracket both included), and their surrounding
  chrome all render crisply doubled rather than blurry or misaligned, with
  hit-testing (clicking, dragging) still lining up correctly with what's
  drawn -- exactly what the logical/physical split above predicts, since
  nothing about *where* things are was ever supposed to change, only how
  many physical pixels render them.

  Original reasoning, still the basis for why fractional stays deferred:
  Wayland's own scale model is integer-only at the protocol level
  (`wl_surface`/`wl_output` only expose whole-number `buffer_scale`); the
  fractional scaling real desktops offer, e.g. 150%, is
  `wp_fractional_scale_v1` layered on top, which still has the client
  render at the next integer scale up and lets the compositor's own
  viewporter do the final fractional crop/downscale.
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
- ~~Move-grab-only output reassignment~~ resolved: `reset_cursor_mode`
  (`core/main.c`) now reassigns a toplevel's output on a resize-grab end
  too, not just a move -- and for both, by which output contains the
  center of the toplevel's own box (a new `output_for_toplevel` helper)
  rather than the cursor's exact position at release. The cursor sits
  wherever the user happened to grab a title bar or resize handle, which
  isn't necessarily representative of where the bulk of the window
  actually ended up; the window's own center is. This also means a small
  resize that only nudges one edge across a boundary doesn't reassign
  anything -- the center has to actually cross too -- which is what
  makes extending this to resize safe: the original concern that
  "resizing across a boundary isn't moving monitors" only really applied
  to an edge grazing the boundary, not to a resize substantial enough to
  relocate the window's center, which is just as much "now it lives over
  there" as a drag is.

  Live-testing this exposed a real limitation of `WLR_WL_OUTPUTS=2`
  nested-backend testing itself, not a bug: a held-button drag can't
  actually cross from one nested output's host window into the other's
  at all -- confirmed as an environment artifact, not code, since it
  persisted with the two host windows positioned edge-to-edge (no gap)
  and reproduced with no window involved, just a plain click-hold-drag.
  Each nested output is its own separate host-level window with its own
  pointer device, absolute coordinates clamped to that window's own
  bounds; the host compositor keeps a held-button drag routed to
  whichever one it started in for as long as the button is down,
  regardless of where the pointer physically travels meanwhile. Real
  multi-monitor hardware has no such split to hit a wall against -- one
  continuous pointer device spans every monitor already. This means a
  *resize* grab crossing a boundary can't be directly exercised in this
  environment (a resized edge is a direct function of the capped cursor
  position, with no way around the wall), unlike a *move*, which can
  still indirectly confirm the fix: dragging by a header offset close to
  the window's own edge can push the window's center past the boundary
  even while the cursor itself is capped at it, and this was confirmed
  live -- the window ended up correctly reassigned to the far output
  (workspace membership, panel highlighting) while its header, a plain
  subsurface rendered whichever output it visually straddles into, sat
  on the near one -- exactly the "leave a straddling window's decoration
  wherever it visually is; keep only the *workspace* bookkeeping
  consistent" behavior already intentional for a drag. The resize case
  shares the identical `reset_cursor_mode`/`output_for_toplevel` code
  path this move case just indirectly verified, so it's correct by
  construction even without being independently exercised live.
- ~~Icon tray / minimize gap~~ resolved: investigating a question about
  the icon row along the bottom of the reference screenshots (see
  `screenshots/sunos551-ow1-scr-01/02/03.png`, which catch Calendar
  Manager and a Text Editor window transitioning between open-window and
  icon form across the same session) confirmed those are iconified
  (minimized) windows sitting loose on the desktop, not app-launcher
  shortcuts -- launching is the root menu's job, already implemented.
  OPEN LOOK's own vocabulary splits what most window managers call
  "close" into two: `Close` iconifies (the app keeps running), `Quit`
  actually terminates it. `shell/src/main.rs`'s `WindowMenuAction::Close`
  (renamed `Minimize` -- the variant now names the action, the row label
  stays "Close" per the reference, same convention `ToggleMaximize`/"Full
  Size" and `Lower`/"Back" already established) now calls
  `set_minimized()` on the toplevel's own foreign-toplevel-management
  handle instead of `close()`; `Quit` is now the only real terminate
  path. No protocol or olcore change needed for this half at all --
  `set_minimized`/`unset_minimized`/`activate` are already standard
  wlr-foreign-toplevel-management requests any bound client (not just
  the owning one) can send, and olcore's `toplevel_handle_request_minimize`
  (tracking `olc_toplevel::minimized` and the visibility logic honoring
  it) already existed from whenever some *client* asked to minimize
  itself -- olshell driving the exact same request from the window menu
  just needed to start calling it.

  The icon tray itself turned out to need no protocol addition either:
  every fact it needs (which toplevels are minimized, which output and
  workspace each belongs to) is already tracked client-side via
  wlr-foreign-toplevel-management's own state array and the workspaces
  protocol's `toplevel_workspace` event. `Olshell::minimized_toplevels_for_output`
  mirrors `toplevel_is_visible`'s combinator inverted (minimized, and
  either sticky or on the output's own active workspace) using data
  already in `self.toplevels`. Icons are drawn directly into each
  output's own background (`draw_background`), the same way a real OPEN
  LOOK icon sits right on the desktop/root window rather than in a
  separate dock or taskbar surface -- bottom-anchored, left to right, one
  row (no wrap yet -- a rare enough number of simultaneously-minimized
  windows on one output that it isn't worth the complexity), each a
  plain bordered box (no attempt at OPEN LOOK's real icon chrome, or a
  per-app glyph -- just the app's first letter and its title/app_id as a
  label below, same "plain and functional" level of fidelity the
  workspace strip already set for anything without a concrete reference
  to match) with hover feedback matching the workspace strip's own.
  ~~SELECT-clicking an icon calls `unset_minimized()` and then
  `activate()` on the same handle, restoring and refocusing it in one
  click -- there's no OPEN LOOK precedent for exactly this gesture (a
  real icon would open its own small menu with an `Open` item instead),
  but a single click is the simplest thing that could work.~~ superseded:
  see the "Icon restore gesture" entry below -- single click now
  selects/highlights, double-click restores, matching authentic OPEN
  LOOK.

  Deliberately deferred, not silently dropped: the icon tray doesn't wrap
  to a second row if it fills up one output's width, and icons show no
  live content the way a real OPEN LOOK clock or calendar icon could (see
  the sunos551 screenshots above) -- real follow-up work if it turns out
  to matter in practice, not attempted here.

  ~~No per-icon menu (Open/Move/Properties, matching a real OPEN LOOK
  icon's own popup)~~ resolved: MENU (right-button) on an icon now opens
  an `IconMenu` (`shell/src/main.rs`) instead of the root menu, a plain
  subsurface of the icon's own `BackgroundOutput` layer surface (same
  "any of olshell's own surfaces can parent a subsurface" trick the
  window menu already relies on, just with the background as parent
  instead of a decoration header), with keyboard focus via
  `grab_keyboard` so Escape closes it, same as the window menu and for
  the same reason. `Open` restores the toplevel -- the same action a
  double-click on the icon already triggers, factored into a shared
  `restore_toplevel` helper. `Properties` is disabled, matching the
  window menu's own convention for the same not-yet-implemented item.
  `Move` is the interesting one: it arms a click-to-follow move ended by
  the *next* press anywhere, the same click-to-arm/click-to-drop pattern
  the window menu's own Move item uses for a real window
  (`zopenlook_decoration_v1::move`'s `held` argument) -- modeled as an
  `IconDrag` with a new `armed: true` flag rather than a separate
  mechanism, since it's the same "follow the pointer, then stop"
  behavior a real press-and-hold drag already has, just started and
  ended differently. Two wrinkles that behavior change surfaced:
  `IconDrag::press_pos` had to become `Option` (`None` until the first
  Motion after arming) since the click that arms a move happens on the
  icon menu's own surface, not necessarily anywhere near the icon --
  establishing the reference point immediately would make the icon jump
  to reflect pointer movement that happened before tracking started;
  and ending on "the next press anywhere" needed a check at the very top
  of `pointer_frame`, before any other press handling, that finalizes
  and fully swallows that confirming press (matching a real window move
  grab, which never delivers its own confirming click to whatever's
  underneath either).

  ~~A per-icon menu is also the one place in olshell an ADJUST-click
  extend-selection gesture (see `docs/OPENLOOK-REFERENCE.md`'s open
  questions) would have a real surface to attach to now -- ADJUST-click
  toggling an icon in/out of a multi-selection without disturbing the
  rest, the way OPEN LOOK's icon lists originally used it. Deliberately
  not built alongside the menu itself: no batch action exists yet to
  consume that selection, so it would be speculative scope with nothing
  to show for it -- still a candidate for later, now that the surface
  for it actually exists.~~ resolved: `BackgroundOutput::selected_icon`
  generalized from a single `Option<ObjectId>` to `selected_icons:
  Vec<ObjectId>`. A plain SELECT-click still replaces it wholesale (same
  single-icon behavior as before); ADJUST (middle-click) on an icon
  toggles just that one icon's membership without touching the rest.
  The batch action that selection needed to be worth building: dragging
  an icon that's already a member of a multi-selection now moves the
  *whole* selection together (`IconDrag::icons`, generalized the same
  way from a single id to a Vec of `(id, origin)` pairs, one drag delta
  applied to all of them, each clamped to the background independently);
  MENU-clicking a member opens the icon menu scoped to the whole
  selection (`IconMenu::group`) instead of just the clicked icon, so
  `Open` restores every selected icon at once and `Move` arms a group
  move. MENU-clicking an icon that *isn't* a member replaces the
  selection with just that one first, matching a plain SELECT-click's
  own replace behavior, so the menu's batch actions never silently act
  on a stale selection the user didn't intend. Confirmed live, with a
  pleasant side effect the same as `icon_position`'s own note above and
  for the identical reason: since `selected_icons` is keyed by the
  toplevel's ObjectId rather than tray position, restoring a whole
  multi-selected group via the icon menu's `Open` and later re-
  iconifying those windows individually brings them back into the tray
  still marked as members of that same selection, rather than resetting
  -- unplanned, but reads as expected selection permanence rather than a
  bug.
- ~~Icon restore gesture~~ resolved: authentic OPEN LOOK/olwm icons are
  double-click (SELECT) to restore, not single-click -- a single click
  just selects/highlights, and a real icon's own MENU-click popup
  (`Open`, `Move`, `Properties`, `Quit`) is the discoverable alternative
  to remembering the double-click. olwc's icon tray now matches: SELECT
  on an icon selects/highlights it (a distinct fill color,
  `ICON_SELECTED_COLOR`), and a second SELECT within
  `ICON_DOUBLE_CLICK_MS` (400ms, measured off the Wayland pointer Press
  event's own timestamp rather than wall-clock time) on the *same* icon
  restores it; clicking elsewhere on the background clears the
  selection. Selection is tracked by the toplevel's `ObjectId`
  (`BackgroundOutput::selected_icon`/`last_icon_click` in
  `shell/src/main.rs`) rather than tray position, since the tray is
  sorted by title and re-sorts as windows are minimized/restored -- a
  position-keyed selection could silently end up highlighting a
  different icon after such a reshuffle. No per-icon MENU-click popup
  yet (see the icon tray entry above), so selecting an icon has no
  effect beyond the highlight for now.
- ~~Icon free positioning~~ resolved: icons are freely drag-
  repositionable to anywhere on the desktop in authentic OPEN LOOK/olwm
  -- olwm's `Olwm*IconLocation`/`IconRegion` resources only ever set the
  *default* placement, same as any other window. olwc's icon tray now
  matches: `ToplevelInfo::icon_position` (`shell/src/main.rs`) holds a
  dragged icon's top-left, background-surface-local; `None` (the default
  for every icon until dragged) falls back to the original packed
  left-to-right layout, with only the *other* undragged icons filling in
  around it. `Olshell::icon_layout()` computes the on-screen rect for
  every icon in the tray this way, shared by drawing, hit-testing, and
  drag start so none of them can disagree about where an icon actually
  is. A press on an icon doesn't commit to being a click or a drag until
  it either moves past `ICON_DRAG_THRESHOLD` (4px) or is released still
  under it (`IconDrag` tracks the ambiguity in between) -- past the
  threshold the icon follows the pointer, clamped to stay fully within
  the background's bounds; released before it, the existing single-
  select/double-click-restore handling from the previous entry runs
  unchanged. A dragged position resets to the default layout if the
  toplevel later moves to a *different* output (via `ToplevelWorkspace`),
  since a stored position only makes sense relative to the output it was
  set on -- carrying it over unchanged could easily land it off-screen on
  a differently-sized output. Confirmed live as a pleasant, unplanned
  side effect: iconifying a window a second time restores it to wherever
  it was last dragged rather than always snapping back to the default
  slot, which reads as expected per-window position memory even though
  nothing was deliberately built to provide it -- it falls straight out
  of `icon_position` living on the toplevel's own long-lived
  `ToplevelInfo` rather than being tray-layout state.

  Dragging surfaced a real bug, not a design question: redrawing the
  *entire* background on every single pointer Motion event (needed so
  the icon visibly follows the pointer) fires far more often than the
  display can present frames, and did so testing live -- badly enough
  that the flood of outgoing Wayland messages overran olcore's
  per-client write buffer, which responded by disconnecting olshell as
  a misbehaving client ("Data too big for buffer" / "error in client
  communication" in olcore's log), taking down the whole nested session.
  Fixed with the standard Wayland throttling idiom: `Olshell::
  request_background_redraw()` requests a `wl_surface.frame` callback
  alongside every redraw and defers any further redraw asked for before
  that callback fires (`CompositorHandler::frame`, previously an unused
  stub, drains the deferred one when it arrives) -- caps redraws to one
  per compositor frame regardless of how fast input events arrive. Every
  caller that used to call `draw_background` directly now goes through
  this instead, not just the drag path, since none of them have a good
  reason to bypass the throttle.

  Explicitly out of scope, not silently dropped: dragging an icon across
  an output boundary. Unlike a toplevel move grab (handled compositor-
  side in olcore, where the cursor position is global), an icon's drag
  is tracked entirely client-side against one output's own background
  surface -- olcore has no idea icons exist at all. Carrying a drag from
  one background surface to another would need olshell to notice the
  Leave/Enter pair on two different `BackgroundOutput`s mid-drag and
  hand the `IconDrag` state across them, which the current per-surface
  press/motion/release handling doesn't attempt; a drag that reaches the
  edge of its starting output's background today just clamps there, the
  same as any other out-of-bounds attempt.
- ~~Icon thumbnail glyph: a postage-stamp-sized static screenshot of the
  window, taken at the moment it's iconified, in place of the current
  generic first-letter glyph.~~ superseded by a different, more authentic
  fix: a live window screenshot was never an OPEN LOOK convention (real
  icons were simple bitmap glyphs, an app-supplied pixmap or a generic
  default, never a snapshot of window content -- window-thumbnail icons
  postdate OPEN LOOK by decades), and would have needed a new olcore-side
  screencopy-style protocol just to read another client's buffer
  contents. Using each app's own real icon in place of the generic
  first-letter glyph gets the same practical win (a tray full of
  identical letter tiles becomes visually distinct) *and* is the
  authentic behavior this replaced was standing in for all along -- no
  olcore changes needed at all, since it's resolved entirely client-side
  from information olshell already has (`app_id`).

  New `shell/src/icon_theme.rs`: `app_id` -> matching `.desktop` file
  (exact-match search across `$XDG_DATA_HOME`/`$XDG_DATA_DIRS`) -> its
  `Icon=` key -> resolved to an actual file via a bounded, lenient subset
  of the freedesktop Icon Theme spec (no `index.theme` parsing or theme-
  inheritance chains -- just a fixed list of theme roots and sizes tried
  directly, same "lenient subset, not full reimplementation" philosophy
  `menu.rs`'s own `.openwin-menu` parser already takes). PNG only, a new
  `image` crate dependency (`default-features = false, features =
  ["png"]`, a small, focused tree -- no SVG rasterizer, a much heavier
  dependency this project's lean-toolkit ethos argued against). An app
  with no icon of its own, or one this can't find, falls back to the
  standard `application-x-executable` icon through the exact same
  pipeline (no special-casing, and no licensing question the way hand-
  picking a specific logo as the fallback would raise) -- only the
  fallback *itself* being unfindable ever drops back to the original
  first-letter glyph now, as an ultimate safety net.

  `ICON_THEMES` searches `hicolor` (the spec's own universal fallback,
  guaranteed on any conformant install) plus `AdwaitaLegacy`/`Adwaita`,
  found while testing this on the project's own dev system to carry PNG
  copies hicolor itself lacked for icons like Konsole's own
  (`utilities-terminal`) and the `application-x-executable` fallback --
  not a spec requirement, just an empirically useful pair. Real gap this
  leaves, called out going in and confirmed rather than glossed over:
  PNG-only coverage is genuinely incomplete, since many modern themes
  (Breeze, current non-Legacy Adwaita) ship a given icon only as SVG --
  those fall through to the generic fallback or the letter glyph rather
  than the app's own icon. `draw_icon_image` scales uniformly to fit (an
  app icon is almost never square, so independently stretching each axis
  would distort it, the same reasoning `draw_glyph_bitmap` already
  follows) and alpha-composites over the icon box's own fill color.
  Verified end-to-end against the real filesystem (not just synthetic
  test fixtures): resolved and rendered Konsole's actual icon, confirming
  the whole pipeline -- desktop-entry lookup, theme resolution, PNG
  decode, scale-to-fit, alpha-composite -- works correctly before ever
  wiring it into a live redraw.
- ~~Window-menu keyboard accelerators: olwm has a real "Mouseless"/menu-
  accelerator system (`clients/olwm/evbind.c`, `menu.c` in the
  historical XView/olwm source -- see the window gadget chrome entry's
  OLGlyph paragraph for where that source lives), with actual
  configurable key bindings that surface as the accelerator-key hints
  shown next to some window-menu entries in the reference screenshots
  (e.g. `Close` paired with a `W`-style hint, `Quit` with `⇧Q`) -- not
  decorative labels, a real bound shortcut.~~ resolved: the "⇧Q" reading
  was a misidentification, corrected below, but the rest held up --
  `core/main.c` already had exactly the extension point needed,
  `handle_keybinding` (previously just `Alt+Escape` to quit the
  compositor, boilerplate carried over from the wlroots tutorial this
  project was bootstrapped from). Generalized it to also take the active
  modifier mask, and check the focused toplevel (`focused_toplevel`, the
  same resolve-from-focused-surface pattern `refocus_if_hidden` already
  used, now shared) against five `Super+<key>` bindings: `W` Close
  (minimize), `F` Full Size (toggle maximize), `B` Back (lower), `S`
  Stick (toggle sticky), `Q` Quit. Super, not Alt, both because Alt is
  already heavily claimed by applications and because Super is the
  modifier most modern desktops already reserve for window-management
  shortcuts (the same reasoning `docs/OPENLOOK-REFERENCE.md`'s
  ADJUST-button entry went through for a different button) -- a modern-
  hardware reinterpretation of olwm's own accelerator mode, not a literal
  port of it. Move/Resize (interactive grabs, not a single-keypress
  action) and Move to Workspace (needs a target workspace, not just a
  bare keypress) are deliberately not bound. The five state-changing
  actions (`toplevel_set_minimized`/`_maximized`, `toplevel_lower`,
  `toplevel_toggle_sticky`, `toplevel_quit`) are now shared functions
  called by both the keybinding and the pre-existing
  wlr-foreign-toplevel-management/openlook-decoration request handlers
  that used to contain this logic directly -- the keybinding needed the
  same effects those already produced, and olcore is the compositor
  running both, so calling internal functions directly needed no new
  protocol surface at all.

  A live-testing exchange corrected something more interesting than a
  bug: what looked like Close showing a plain `W` hint but Quit showing
  a Shift-prefixed `⇧Q` turned out, on closer investigation prompted by
  the user's own memory of a diamond-like glyph in the screenshots, to
  be exactly one modifier throughout, not two different ones per item.
  Confirmed from source: real olwm accelerators are single-modifier,
  bound to the physical "Meta" key Sun keyboards marked with a diamond
  glyph (`evbind.c`'s actual defaults are plain `w+Meta`/`q+Meta`, and
  `menu.c`/`ol_button.c`'s `olgx_draw_diamond_mark` -- a small six-point
  outline drawn procedurally, not a bitmap font character -- draws that
  diamond next to the accelerator letter whenever a binding includes
  Meta). What read as a Shift-arrow before the Q was that same diamond,
  easy to misidentify at screenshot resolution. Fixed on both sides:
  Quit's binding changed from Super+Shift+Q to plain Super+Q, matching
  the other four; and the window menu now draws a small hand-drawn
  filled-diamond bitmap (`DIAMOND_MARK_GLYPH`, `shell/src/main.rs` --
  hand-drawn rather than OLGlyph-traced, since the original was
  procedural too, not a font glyph) before each accelerator letter,
  rather than a bare letter or a spelled-out modifier name.

  Confirmed live that the code path runs without error (window opens,
  minimizes, no crashes), but functional verification of the shortcuts
  themselves hit a real, un-fixable-on-our-end wall: testing happens in
  a nested olcore running as an ordinary window inside the user's own
  KWin session, and KWin itself intercepts every one of these Super+<key>
  combos for its own global shortcuts before the nested compositor ever
  sees them -- an artifact of testing a compositor-inside-a-compositor,
  not a bug in this feature. The underlying mechanism is the same code
  path the pre-existing Alt+Escape binding already used successfully,
  just generalized, so this is expected to work correctly on a session
  where the host doesn't grab Super combos (bare metal, a VT-switched
  session, or a host with those particular shortcuts freed up) -- worth
  remembering as a standing limitation of this project's nested-in-KWin
  testing setup, not something to keep re-discovering.
- ~~Pill-shaped menu-item highlight: hovering a window-menu item (or a
  root-menu item) currently fills a plain rectangle
  (`MENU_HOVER_COLOR`); authentic OPEN LOOK uses an obround/pill shape
  instead, per both reference screenshot families.~~ resolved: traced
  from the same OLGlyph font (`olgl14.bdf`) as the button/pushpin/arrow
  glyphs, this time `ol_button.c`'s stretchable-button encodings (24-29
  for the two endcaps, 30/35/40 for the tileable middle segments --
  `BUTTON_UL`/`_LL`/`_LEFT_ENDCAP_FILL`/`_LR`/`_UR`/`_RIGHT_ENDCAP_FILL`/
  `_TOP_1`/`_BOTTOM_1`/`_FILL_1`). Confirmed from source that `olwm`'s
  window menu and XView's own menu widget both call the same
  `olgx_draw_accel_button` (`libolgx`) for this, so there's no "Sun vs
  olvwm" design split to arbitrate -- one shape both share. What *does*
  differ between the two reference screenshots -- olvwm's beveled/
  recessed fill vs. the Sun screenshot's flat black outline -- is the
  same `info->three_d` runtime 2D/3D split found twice earlier this
  session (the pushpin's flat-vs-bevel-composite glyphs, the resize
  corners): `olgx_draw_accel_button`'s `OLGX_INVOKED` state fills with a
  beveled pill (`BG3` top / `WHITE` bottom / `BG2` fill) in 3D mode, or a
  single solid black outline in 2D mode. Built the 3D beveled version
  (consistent with the raised/inset bevel convention already used
  throughout olshell's chrome), mapped to the same
  `DECORATION_BEVEL_DARK`/`_LIGHT` colors the decoration header's own
  focus bevel uses rather than introducing new ones, with
  `MENU_HOVER_COLOR` kept as the fill so the existing look isn't changed
  along with the shape; the flat 2D version stays a concrete first
  candidate for the theming entry above once that exists.

  Genuinely a bigger lift than the fixed-size button/pushpin/arrow
  glyphs already traced, since a menu-item highlight has to stretch to
  whatever width a row's text needs rather than blit once at a fixed
  size -- `draw_pill_highlight` (`shell/src/main.rs`) uses OPEN LOOK's
  own technique for this: fixed endcap glyphs plus a 1-native-pixel-wide
  middle glyph (`PILL_TOP_TILE`/`_BOTTOM_TILE`/`_FILL_TILE`) repeated
  exactly `needed_width - 2*endcap_width` times via a new `blit_bitmap`
  primitive (unlike `draw_glyph_bitmap`'s smooth aspect-fit scaling used
  for the fixed-size glyphs, blitting at native pixel size times `scale`
  only) -- since the tile is exactly 1 pixel wide, this always divides
  evenly, with no fractional-tiling remainder to handle the way a smooth
  scale factor would leave. Three layers (top_color/bottom_color/
  fill_color, each an endcap-tiles-endcap run) composited at the same
  origin, matching `ol_button.c`'s own three-`XDrawText`-calls approach;
  the fill layer is drawn last but never overwrites the outline above it,
  since (confirmed by computing each glyph's absolute row position from
  its own BDF `BBX` height/`yoff` pair) the fill glyphs are one native
  pixel inset from the outline on every edge by construction.

  Live testing surfaced a real, if minor, bug: centering the pill purely
  on its own native height within the row left it sitting visibly higher
  than the text drawn over it -- invisible against the flat rectangle
  this replaced, obvious once there was a shape with a visible top/bottom
  edge to compare against. Rather than guess a pixel offset from a
  screenshot, rendered the real glyph and the real text together (a
  throwaway `#[test]` exercising the actual functions, removed before
  committing) and compared their pixel centers directly: text sat about
  1.5 logical pixels below the pill's geometric center, closed with a
  small documented `PILL_VERTICAL_BIAS` constant. A second bug from the
  same testing pass: the root menu's own hover highlight (`draw_popup`)
  turned out not to have been converted to `draw_pill_highlight` at all
  on the first attempt, still showing the old flat rectangle -- a
  same-text-different-indentation `replace_all` edit silently skipped it
  since its exact leading whitespace didn't match the other three
  (differently-nested) call sites it was written against, a real gap
  worth remembering to double-check with a fresh grep rather than trusting
  a "successfully replaced" result to mean *every* intended call site.
