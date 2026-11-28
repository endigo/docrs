# Font provenance

## Inter

- `inter/Inter[opsz,wght].ttf`: Inter 4.001 (`Version 4.001;git-66647c0bb`), the variable roman with optical size and weight axes. The italic is not included: the PDF fixtures that use Inter set no italic.
- `inter/OFL.txt`: its licence, the SIL Open Font License 1.1.
- Source: https://github.com/google/fonts, `ofl/inter/`, commit `0b58fb370093f9a9f4ff785d94405710b79de67c` (upstream https://github.com/rsms/inter, commit `66647c0bbbe41a850d79d9c76fb13add3378940f`). Both files are unmodified; their git blob hashes match the ones in google/fonts.

| File | SHA-256 |
| -- | -- |
| `Inter[opsz,wght].ttf` | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |
| `OFL.txt` | `5b9321a4298cfeb6b34354164a1c3afc3db114569984c502b9b35d988fd58c57` |

The PDF fixtures in `tests/fixtures/pdf` lay out and later draw with it. Install it before exporting those fixtures through Word (`tools/pdf/README.md`). `crates/docrs/tests/pdf_fonts.rs` embeds its Regular and Bold instances.

## Liberation Serif

- `liberation/LiberationSerif-Regular.ttf`: Liberation Serif 2.1.5 (`Version 2.1.5`), a static TrueType font with a `kern` table. `crates/docrs/tests/pdf_fonts.rs` embeds it, and `tests/fixtures/pdf/font-line.pdf` is drawn with it.
- `liberation/LICENSE`: its licence, the SIL Open Font License 1.1.
- Source: https://github.com/liberationfonts/liberation-fonts, release 2.1.5, `liberation-fonts-ttf-2.1.5.tar.gz` (SHA-256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`). Both files are unmodified.

## Source Sans 3

- `source-sans/SourceSans3-Regular.otf`: Source Sans 3 Regular 3.052 (`Version 3.052;hotconv 1.1.0;makeotfexe 2.6.0`), an OpenType font with CFF outlines. `crates/docrs/tests/pdf_fonts.rs` embeds it.
- `source-sans/LICENSE.md`: its licence, the SIL Open Font License 1.1.
- Source: https://github.com/adobe-fonts/source-sans, branch `release`, commit `87b37a2daaed80fcb8e8ccb0085c4d72ddade12e`, `OTF/SourceSans3-Regular.otf` and `LICENSE.md`. Both files are unmodified.

| File | SHA-256 |
| -- | -- |
| `liberation/LiberationSerif-Regular.ttf` | `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74` |
| `liberation/LICENSE` | `93fed46019c38bbe566b479d22148e2e8a1e85ada614accb0211c37b2c61c19b` |
| `source-sans/SourceSans3-Regular.otf` | `08df266400933d3178d081a45f94a08814c3e55b4b7dd2e0ff69cb1329f13ab6` |
| `source-sans/LICENSE.md` | `56af9b9c6715597e458284a474dc118a50a4150e9d547c70f7b4a33c3e6a9328` |
