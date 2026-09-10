# olwc — an OPEN LOOK Wayland compositor

olwc recreates the look and feel of **OPEN LOOK** — the GUI of Sun's
OpenWindows, as rendered by the `olwm` / `olvwm` window managers — as a
modern Wayland compositor built from scratch.

It is not a port. The original XView/OLIT/olvwm code is a *reference
only*; olwc is a clean-room reimplementation. But it is a careful one:
the chrome glyphs are traced pixel-for-pixel from Sun's own OLGlyph
bitmap font, the menu grammar follows `olvwm`'s real `.openwin-menu`
parser, the window-menu items and their two-state labels come straight
out of `usermenu.c`, and dozens of smaller behaviors were checked
against the historical C source and period OpenWindows screenshots
rather than from memory. `docs/DESIGN.md` is a running log of that work,
decision by decision.

<!--
  TODO: add a screenshot of olwc running.
  Suggested: docs/media/screenshot.png, then reference it here:

  ![olwc running](docs/media/screenshot.png)
-->

## What works

**Window management**

- Server-side window decoration (custom `openlook-decoration` protocol):
  header with the abbreviated menu button, a bold *Window* / centered
  title, the focused window's recessed title-bar panel, right-angle
  resize brackets at all four corners plus a footer strip, a plain
  black border frame, and a pushpin indicator on sticky windows.
- Window menu (right-click the title bar or the button): **Close**
  (iconify), **Full Size** / **Restore Size**, **Move**, **Resize**,
  **Properties** (stub), **Back** (lower), **Stick** / **Unstick**,
  **Move to Workspace** (submenu), **Quit** (close every window of the
  app). Keyboard navigation with the OPEN LOOK location cursor, plus
  `Super`+key accelerators shown with the OLGlyph diamond mark.
- Real maximize / restore, interactive move and resize.

**Root menu**

- `~/.openwin-menu` parser: `TITLE`, `exec`, `MENU`/`END`, `EXIT`,
  `REREAD_MENU_FILE`, `INCLUDE`, `DEFAULT`, `SEPARATOR` — plus one
  olwc-specific keyword, `APPMENU`, that builds a category-grouped
  Programs submenu from the system's `.desktop` files (the modern
  stand-in for `olvwm`'s dynamic `DIRMENU`).
- The pushpin "pin to persist" gesture, interactive nested submenus,
  keyboard "Mouseless" navigation, on-screen clamping near edges, the
  3D box frame, the bold title.
- `Exit…` opens a confirmation Notice with the default-button ring and
  the pointer warped onto it, matching `olvwm`'s `PopupJumpCursor`.

**Workspaces**

- Per-output linear workspaces (custom `openlook-workspaces` protocol) —
  discrete and swipeable, replacing OPEN LOOK's pannable Virtual Desktop
  Manager. A workspace switcher strip per output; ADJUST-click a strip
  segment to send the focused window there.

**Icons (minimized windows)**

- Minimized windows become icons on the desktop. `Close` iconifies (the
  app keeps running); a second click on the icon restores it. Per-icon
  menu, free drag-positioning, multi-select with ADJUST, application
  icons resolved from `.desktop` files and icon themes, and the
  iconify/deiconify "zoom lines" flash (`olvwm`'s
  `DrawIconToWindowLines`).

**Rendering**

- Chrome drawn from OLGlyph glyphs traced from `olgl14.bdf`, using the
  same three-layer bevel technique as `libolgx`'s 3D mode.
- **Luxi Sans** for all UI text — Bigelow & Holmes' own open
  reimplementation of the Lucida Sans that OpenWindows used, regular
  and a real bold weight.
- Integer HiDPI scaling (`wl_surface.set_buffer_scale`).

## Deliberately not done

Each of these was a decision, not an omission — see `docs/DESIGN.md`:

- **The pannable Virtual Desktop Manager.** Replaced by linear
  workspaces.
- **`Refresh`** (window and root menu). It existed to force-repaint a
  stale X11 window; Wayland's damage-tracking model makes that class of
  bug structurally impossible, so there is nothing to wire it to.
- **`Restart WM`.** X11 reparents every client to the root window and
  re-`exec`s in place; a Wayland client's connection belongs to the
  compositor's own `wl_display`, so there is no hand-off path.
- **OPEN LOOK scrollbars** and any other widget *inside* a window. Those
  were drawn by the application's toolkit, never by `olwm`/`olvwm`
  itself.

## Not yet

- Window-type differentiation (transient/dialog frames, the
  command-window pushpin model, `MENU_LIMITED` menus).
- Fractional scaling.
- FreeBSD: an architectural target (wlroots' backend supports it), not
  yet tested.

## Architecture

Two processes, split on a privilege boundary:

```
┌──────────────────────────┐         ┌───────────────────────────┐
│  olcore  — C, wlroots    │◄───────►│  olshell  — Rust           │
│  privileged compositor:  │ Wayland │  unprivileged client:      │
│  DRM/KMS, libinput, seat,│  proto  │  every OPEN LOOK pixel —    │
│  compositing, window     │         │  menus, decoration chrome, │
│  placement & focus       │         │  workspace strip, icons    │
└──────────────────────────┘         └───────────────────────────┘
```

`olcore` is a generic, well-behaved wlroots compositor. It knows nothing
about OPEN LOOK's *look* — all of that lives in `olshell`, which talks to
`olcore` only over Wayland protocol (no shared memory, no side channel).
Beyond the standard extensions (`wlr-layer-shell`,
`wlr-foreign-toplevel-management`, `xdg-shell`), olwc adds three small
protocols of its own, in `protocol/`:

| protocol | for |
|---|---|
| `openlook-decoration-unstable-v1` | attaching header/resize chrome to another client's window; the iconify flash |
| `openlook-workspaces-unstable-v1` | per-output linear workspaces |
| `openlook-session-unstable-v1` | ending the session (the `Exit…` item); pointer warp for the Notice |

```
core/       olcore — C, meson
shell/      olshell — Rust, cargo
protocol/   the three custom .xml protocol definitions
docs/       DESIGN.md (the running design log) and OPENLOOK-REFERENCE.md
screenshots/ period OpenWindows / olvwm screenshots, used as reference
```

## Building

**olcore** needs a C toolchain, meson ≥ 0.59, and:
wlroots 0.18–0.20, wayland, wlr-protocols, wayland-scanner, xkbcommon,
libinput, pixman.

```sh
meson setup core/build core
ninja -C core/build
```

**olshell** needs a recent stable Rust (2021 edition):

```sh
cargo build --manifest-path shell/Cargo.toml
```

## Running

`olcore` launches `olshell` itself via `-s`:

```sh
./core/build/olcore -s "./shell/target/debug/olshell"
```

To try it nested inside an existing Wayland session (a window on your
current desktop):

```sh
WLR_BACKENDS=wayland ./core/build/olcore -s "./shell/target/debug/olshell"
```

Right-click the background for the root menu. Set `OLWC_MENU` to point at
a menu file, or drop one at `~/.openwin-menu`; with neither, a small
built-in menu is used.

> **Note on the nested backend:** running inside another compositor,
> `olcore` sees the host pointer position as absolute, so a couple of
> effects that rely on warping the software cursor (the Notice's
> jump-to-default-button) are only fully visible on a real
> DRM/libinput session.

## Reference material

- **`docs/DESIGN.md`** — the chronological design log: every feature,
  the source it was checked against, the bugs found along the way, and
  what was deliberately left out.
- **`docs/OPENLOOK-REFERENCE.md`** — the OPEN LOOK interaction rules and
  widget vocabulary distilled into a checklist, cross-referenced to the
  XView/OLIT source.

The historical XView and `olvwm` source used as reference lives at
[github.com/MagnetarRocket/xview-openlook](https://github.com/MagnetarRocket/xview-openlook)
and in the FreeBSD ports distfiles.

## License

MIT — see [`LICENSE`](LICENSE). Bundled and derived-from third-party
assets (the Luxi Sans fonts; the provenance of the traced OLGlyph
chrome) are listed in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

OPEN LOOK and OpenWindows were trademarks of Sun Microsystems / AT&T.
olwc is an independent homage and is not affiliated with, endorsed by,
or derived from the code of Oracle or any successor.
