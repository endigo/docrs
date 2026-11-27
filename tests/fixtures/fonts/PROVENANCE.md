# Font provenance

## Inter

- `inter/Inter[opsz,wght].ttf`: Inter 4.001 (`Version 4.001;git-66647c0bb`), the variable roman with optical size and weight axes. The italic is not included: the PDF fixtures that use Inter set no italic.
- `inter/OFL.txt`: its licence, the SIL Open Font License 1.1.
- Source: https://github.com/google/fonts, `ofl/inter/`, commit `0b58fb370093f9a9f4ff785d94405710b79de67c` (upstream https://github.com/rsms/inter, commit `66647c0bbbe41a850d79d9c76fb13add3378940f`). Both files are unmodified; their git blob hashes match the ones in google/fonts.

| File | SHA-256 |
| -- | -- |
| `Inter[opsz,wght].ttf` | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |
| `OFL.txt` | `5b9321a4298cfeb6b34354164a1c3afc3db114569984c502b9b35d988fd58c57` |

The PDF fixtures in `tests/fixtures/pdf` lay out and later draw with it. Install it before exporting those fixtures through Word (`tools/pdf/README.md`).
