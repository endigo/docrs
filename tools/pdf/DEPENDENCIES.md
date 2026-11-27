# PDF engine: dependency measurements

Measured 2026-11-27 for the PDF export work. Nothing here is a workspace
dependency yet. Rerun the probe before acting on these numbers.

## How

The probe was a scratch `cdylib` outside the workspace. It depends on
`crates/docrs-wasm` by path, and each candidate crate sits behind a feature.
One exported function calls the candidate, so the linker keeps it. The probe
was built for `wasm32-unknown-unknown` with `cargo build --release`, which is
the workspace's release profile with no overrides, then run through
`wasm-bindgen --target nodejs` 0.2.129. Sizes are those of `*_bg.wasm`:
raw bytes, then `gzip -9` bytes. Toolchain: stable 1.97.1, and each candidate
also built alone with 1.88.0.

Baseline: `crates/docrs-wasm/build.sh` gives `docrs_bg.wasm` of 3,279,995 bytes
raw and 1,292,847 gzip. The probe's own baseline, docrs-wasm plus the probe
function, is 3,281,978 raw and 1,283,844 gzip. The deltas below are against
the probe baseline.

## Sizes

| Added to docrs-wasm | raw bytes | gzip bytes | Δ raw | Δ gzip |
| --- | ---: | ---: | ---: | ---: |
| nothing (baseline) | 3,281,978 | 1,283,844 | | |
| `subsetter` 0.2.6, no default features | 3,401,605 | 1,329,932 | +119,627 | +46,088 |
| `subsetter` 0.2.6, `variable-fonts` | 4,602,280 | 1,615,194 | +1,320,302 | +331,350 |
| `png` 0.18.1, decoder only | 3,368,571 | 1,317,231 | +86,593 | +33,387 |
| `harfrust` 0.16.2, `std` | 4,537,355 | 1,690,392 | +1,255,377 | +406,548 |
| `subsetter` (variable) + `png` | 4,688,347 | 1,648,135 | +1,406,369 | +364,291 |

Release profile with `lto = "fat"`, `codegen-units = 1` and `opt-level = "s"`.
These settings are not in the workspace today.

| Added to docrs-wasm | raw bytes | gzip bytes | Δ raw | Δ gzip |
| --- | ---: | ---: | ---: | ---: |
| nothing (baseline) | 2,471,346 | 1,094,872 | | |
| `subsetter`, no default features | 2,562,556 | 1,123,746 | +91,210 | +28,874 |
| `subsetter`, `variable-fonts` | 3,171,119 | 1,290,870 | +699,773 | +195,998 |
| `png`, decoder only | 2,551,133 | 1,126,171 | +79,787 | +31,299 |

## Crates

| Crate | Licence | MSRV declared | Builds with 1.88 | Adds (normal deps, wasm32) | `unsafe` |
| --- | --- | --- | --- | --- | --- |
| `subsetter` 0.2.6 static | MIT OR Apache-2.0 | 1.85 | yes | `rustc-hash` | `deny(unsafe_code)`; none in `rustc-hash` |
| `subsetter` 0.2.6 `variable-fonts` | MIT OR Apache-2.0 | 1.85 | yes | `skrifa` 0.42, `read-fonts` 0.39, `write-fonts` 0.48, `font-types` 0.11, `kurbo` 0.13, `polycool`, `bytemuck`, `indexmap`, `hashbrown` 0.17, `equivalent`, `smallvec`, `arrayvec`, `rustc-hash` | the font crates are `forbid`/none; `bytemuck`, `hashbrown`, `smallvec`, `arrayvec`, `indexmap` have the usual container `unsafe`; `font-types` has 3 uses |
| `png` 0.18.1 | MIT OR Apache-2.0 | 1.73 | yes | `fdeflate`, `flate2` (shared), `miniz_oxide` 0.9 next to the workspace's 0.8, `adler2`, `crc32fast` (shared), `simd-adler32`, `bitflags` | `png`, `fdeflate`, `miniz_oxide` forbid it; `simd-adler32` has 36 uses |
| `harfrust` 0.16.2 | MIT | 1.85 | yes | `read-fonts` 0.47 (not the 0.39 `subsetter` uses), `font-types` 0.12, `bytemuck`, `smallvec`, `core_maths`, `libm`, `spin`, `yoke`, `zerofrom`, `bitflags`, `stable_deref_trait` (+ `foldhash` with `std`) | `harfrust` forbids it; `spin`, `yoke` have some |

With `default-features = false` alone, `harfrust` does not build. It needs
either `std` or `libm` (`f32::round`/`sin` are missing otherwise). With
`libm` the whole probe is 1,251,559 / 385,659 bytes. With `std` it is
1,295,971 / 393,465.

## Variable font instancing with `subsetter`

The function is `subsetter::subset_with_variations(data, 0,
&[(Tag::new(b"wght"), w)], &remapper)`. It was run on Inter
(`Inter[opsz,wght].ttf`, 876,576 bytes) and Roboto Flex (13 axes,
1,787,292 bytes) from google/fonts, at weights 400 and 700, keeping
printable ASCII and `–—‘’“”…é`, 103 code points:

- The output has `glyf head hhea hmtx loca maxp name post`, plus `prep` at
  400. It has no `fvar`, `gvar`, `HVAR`, `avar`, `STAT` or `cmap`. That is a
  static TrueType program fit for `FontFile2`, and CoreText loads all four.
- Advances: 103/103 of the output's `hmtx` advances equal skrifa's advances
  for the source at that location (maximum difference 0.00 units). Outlines:
  103/103 identical point for point.
- Glyph ids are renumbered: the remapper's order, plus composite components
  (104 requested, 109 or 107 glyphs out). The PDF writer has to use the new
  ids.
- Each call took 0.03–0.6 ms natively. The output is 10.7–11.3 KB.
- `Face::parse` reads only the default instance of a variable font. At
  700 its advances are off by up to 282 units (Inter) and 244 (Roboto Flex)
  per 2048. Measuring a variable font at a weight needs the instanced
  advances.
