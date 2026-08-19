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

- Which specific gadget/icon shapes to source or redraw (pushpin,
  resize corner glyphs) — needs actual asset work, not just spec
  reading.
- Whether ADJUST (middle-click extend-selection) is worth preserving
  given how rarely modern users have a reliable middle-click.
- Root menu content/config format (still open from the design doc).
