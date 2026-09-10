# Third-party notices

olwc itself is under the [MIT License](LICENSE). It bundles or derives
from the following, under their own terms.

## Luxi Sans (bundled)

`shell/assets/fonts/luxisr.ttf`, `shell/assets/fonts/luxisb.ttf`

Luxi Sans Regular and Bold, copyright © 2001 Bigelow & Holmes Inc.;
instruction code copyright © 2001 URW++ GmbH. Redistributed under the
Luxi fonts license (an MIT-style permission grant with a
no-modification clause). Full text in
[`shell/assets/fonts/LUXI-LICENSE.txt`](shell/assets/fonts/LUXI-LICENSE.txt).

Luxi is Bigelow & Holmes' own open reimplementation of the Lucida Sans
family they designed, donated to X.Org in 2001. OpenWindows used
Lucida Sans (`-b&h-lucida-*`) for all UI text; the original B&H Lucida
Sans is proprietary, so Luxi is the closest redistributable match.

## OLGlyph (traced, not bundled)

The window-chrome bitmaps in `shell/src/main.rs` — the pushpin, menu
marks, abbreviated-button housing, default-button ring, accelerator
diamond, resize brackets — are traced pixel-for-pixel from Sun
Microsystems' **OLGlyph** bitmap font (`olgl14.bdf`).

That font is preserved, still under Sun's original 1989 notice
("Permission to use, copy, modify, and distribute this software and
its documentation for any purpose and without fee is hereby
granted…"), in the historical XView source trees at
[github.com/MagnetarRocket/xview-openlook](https://github.com/MagnetarRocket/xview-openlook)
and [github.com/ggodd/xview-64bit](https://github.com/ggodd/xview-64bit)
(`xview-base/fonts/bdf/misc/olgl14.bdf`). No code from those trees is
used — only the glyph pixel data, transcribed by hand.

## Reference material (not incorporated)

`olvwm` and XView source, and OpenWindows screenshots, were consulted
as a behavioral and visual reference for a clean-room reimplementation.
No code, APIs, or implementation details were ported. See
`docs/OPENLOOK-REFERENCE.md`.
