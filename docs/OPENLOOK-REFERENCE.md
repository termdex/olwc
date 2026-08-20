# OPEN LOOK Reference — for olshell look-and-feel work

## Purpose

This document distills the OPEN LOOK GUI specification's interaction
rules and widget vocabulary, as implemented by its two reference
toolkits (XView and OLIT), into a checklist for `olshell`'s design.

**This is a behavioral/visual reference only.** No code, APIs, or
implementation details from XView or OLIT are to be ported or
adapted — `olwc` is a clean-room reimplementation targeting the same
spec, not a port of either toolkit. XView and OLIT are useful here
purely because they're two independent, faithful implementations of
the same underlying spec, which makes them a good cross-check for
"what did OPEN LOOK actually specify" versus folk memory or
screenshots alone.

Note on scope: `olwc` targets the *spirit* of OPEN LOOK (per the
design doc), not full fidelity. Treat everything below as a menu to
pull from selectively, not a compliance checklist.

## Mouse button semantics

OPEN LOOK defined three canonical mouse button roles, independent of
physical button count:

- **SELECT** (left button) — choose/activate an object, the primary
  action.
- **MENU** (right button) — pop up a context-appropriate menu for
  the object under the pointer.
- **ADJUST** (middle button) — extend or modify an existing
  selection (e.g. add to a multi-select) rather than replace it.

Worth deciding early whether `olshell` preserves three-button
semantics as-is, or maps ADJUST onto a modifier+click for modern
two-button/trackpad users — this is a real usability question for
2026 hardware, not just a fidelity one.

## Pushpin menus

OPEN LOOK's signature menu interaction: menus could be "pinned" open
via a pushpin icon in the menu's corner, converting a transient popup
into a persistent, movable window. Two distinct behaviors worth
separating:

- **Press-and-hold / release-to-select** — a menu could be used
  transiently: press MENU, drag to an item, release to select and
  dismiss.
- **Pin-to-persist** — clicking the pushpin icon (or a variant
  gesture) keeps the menu open as a small floating palette after
  release, for repeated use.

This pairing (transient-by-default, pin-to-persist) is one of the
most recognizable and reusable OPEN LOOK interaction ideas — a good
candidate to preserve even under the "spirit, not fidelity" goal.

Three genuine SunOS/OpenWindows screenshots confirm the pushpin's exact
placement: `screenshots/sunos551-ow1-scr-01.png`, `-02.png`, and
`OpenWindows-Augmented-Compatibility-Environment_1.png` (a later,
more-beveled build, useful as a cross-check that the convention held
across versions). In all three, the pushpin sits in the header row's
**top-left corner**, immediately before the menu's title text (e.g. the
"Workspace" and "Programs" menus) -- not top-right. Window title bars in
the same screenshots carry no pushpin at all; only menus get one.
`shell/src/main.rs`'s `MenuPopup::pushpin_rect()` originally placed it
top-right, since it predated having a real reference for this -- fixed to
match.

## Window gadgets (decoration chrome)

Terminology and components olvwm's window frames used, worth
matching in `olshell`'s decoration rendering:

- **Header** — the title bar area.
- **Pushpin** (window-level) — some frame styles included a pushpin
  affordance on the frame itself, not just menus.
- **Resize corners** — distinctive obround/pill-shaped resize
  handles at frame corners rather than thin edge-drag regions.
- **Footer / resize-only region** — a bottom strip dedicated to
  resize, separate from the header.

### Window menu (observed from screenshots)

Two reference screenshots live in `screenshots/` at the repo root:
`Olvwm-desktop.jpg` (olvwm on X11 — a faithful, era-accurate OPEN LOOK
reference) and `Openwindows.jpg` (a later Sun desktop, closer to
CDE/Java Desktop System, with similar window chrome but a more modern
icon-and-label taskbar along the screen bottom that is **not** an OPEN
LOOK convention — don't take that taskbar as a look-and-feel source).

From `Olvwm-desktop.jpg`, with the window menu open on an `xterm`:

- A small button sits in the title bar's far-left corner (a
  downward-pointing chevron glyph in a small square, distinct from
  the pushpin) — SELECT-clicking it pops the window menu below the
  title bar.
- Menu contents, top to bottom: `Close`, `Full Size`, `Move`,
  `Resize`, `Properties` (shown grayed out/disabled — context-
  dependent), `Back`, `Refresh`, `Stick`, `Quit`. Single column,
  left-aligned labels, no icons except accelerator-key hints
  right-aligned on some entries (e.g. `Close` paired with a `W`-style
  hint, `Quit` paired with a `⇧Q`-style hint). `olshell`'s
  implementation drops `Refresh`: it exists to force a repaint of a
  stale X11 window (a common issue in that era, especially over the
  network-transparent X11 setups OpenWindows was often used with),
  which Wayland's damage-tracking model makes structurally impossible
  -- there's nothing left for it to do, so unlike the still-placeholder
  items it's omitted rather than kept as a dead menu entry. `Back`
  (lower the window to the bottom of the stack) is implemented.
- The item under the pointer (`Close`, in the screenshot) is drawn
  with a distinct pill/oblong outline around it — the same "obround"
  shape language the Visual language section below calls out for
  buttons generally, applied here to menu-item highlighting too.
- The title bar itself: centered title text, a subtle light/dark
  bevel for 3D shading, and a clean thin border separating it from
  the content area.
- The "Virtual Desktop" pager window visible in the corner of both
  screenshots is olvwm's VDM (Virtual Desktop Manager) — explicitly
  out of scope per the design doc's non-goals. Useful to see what
  we're deliberately *not* reproducing, not as something to match.

## Widget vocabulary (from OLIT / XView, for naming and behavior reference)

| OPEN LOOK term | Rough modern equivalent | Notes |
|---|---|---|
| Oblong / obround button | Rounded/pill button | The canonical OPEN LOOK button shape — rounded ends, not rectangular. |
| Checkbox | Checkbox / toggle | Had specific exclusive vs. non-exclusive selection semantics worth checking against the spec if implementing grouped options. |
| Abbreviated menu button | Dropdown / combo button | Shows current selection, click to reveal full menu — closer to a modern `<select>` than a full pushpin menu. |
| Scrolled window | Scrollable panel | Combined scrollbar + content-area conventions. |
| Notice | Modal alert / dialog | Distinct from a general dialog — used specifically for short, must-acknowledge messages. |
| Panel | Form / control panel area | A container for grouped controls (buttons, fields), conceptually close to a modern settings panel. |

## Visual language

- **Obround (pill) shapes** — buttons, resize handles, and some
  indicators favor rounded/pill shapes over sharp rectangles.
- **3D beveled shading** — controls used light/dark bevel edges to
  suggest physical depth (raised = unpressed, inset = pressed),
  rather than flat or Motif-style shading conventions.
- **Root menu** — the desktop background itself had a MENU-triggered
  root menu, typically listing available applications and
  workspace/window operations.

## Open questions for later look-and-feel passes

- Window-menu button glyph and pushpin icon shape: the screenshots
  confirm *what* they are and roughly where they sit, but not exact
  pixel proportions, colors, or the chevron glyph's precise shape.
  `shell/src/main.rs`'s `draw_chevron()` is an explicitly placeholder
  geometric approximation (a filled downward wedge), not asset-accurate.
- Resize corner glyphs specifically aren't visible in either
  screenshot at usable resolution -- still open. Bottom-left and
  bottom-right handles exist now (see `docs/DESIGN.md`'s window gadget
  chrome entry) as an explicit placeholder shape (a filled circle, not
  true obround), same caveat as `draw_chevron()` above; top corners and
  a footer strip are still unimplemented.
- ~~Whether to reproduce the full window-menu item list~~ resolved: the
  reference set minus `Refresh` (dropped, see `docs/DESIGN.md`'s window
  gadget chrome entry for why). The menu is fully interactive now, with
  every item either a real action or an intentional placeholder
  (`Properties` disabled to match the screenshot, `Stick` blocked on
  workspace membership existing, which it now does -- see the
  workspace-switcher entry in `docs/DESIGN.md`).
- ~~Whether ADJUST (middle-click extend-selection) is worth
  preserving~~ given a real, if non-authentic, use: ADJUST-click on a
  workspace strip segment moves the focused window there. Whether
  ADJUST is worth preserving for anything closer to its original
  extend-selection meaning is still open.
- ~~Root menu content/config format~~ resolved: olwm-compatible
  `.openwin-menu`, implemented in `shell/src/menu.rs`.
- ~~Window header chrome~~ v1 resolved: see `docs/DESIGN.md`'s window
  gadget chrome entry. The window *menu* itself (as opposed to the
  header it hangs off) is still open.
