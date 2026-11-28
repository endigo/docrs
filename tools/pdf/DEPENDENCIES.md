# PDF engine: dependency measurements

Measured on 2026-11-27 for the PDF export work. Nothing here is a workspace
dependency or a profile change yet. Rerun the measurements before acting on
these numbers.

Machine: Apple M4 Pro, 12 cores, 24 GB, macOS. Rust stable 1.97.1,
`wasm-bindgen` 0.2.129, Node 26.13.1. Other jobs were running, with load
averages between 6 and 17.

## Recommendation

- **Release profile for the wasm build:** fat LTO, `codegen-units = 1`, and
  the default `opt-level = 3`. **Crate type:** `docrs-wasm` as `cdylib`
  only. Cargo does not apply LTO to a crate that is also an `rlib`, and today
  `docrs-wasm` is `cdylib + rlib`. With these settings, docrs-wasm plus
  `subsetter` with variable fonts is 3,796,150 bytes raw, under the 3.9 MB
  budget with about 100 KB spare, and `fromHtml` is a little faster than
  today. `opt-level = "s"` is rejected: it saves another 625 KB but makes
  `fromHtml` 16% slower here, 15% in one review's run and 32% (median) in
  the other's.
- **Headroom:** about 100 KB is thin for the PDF writer, painter and PNG
  decoder still to come. Each of those PRs should measure its own size; if
  they do not fit, the choice is between raising the budget, stripping the
  wasm `name` section, or deferring variable fonts.
- **Made with `subsetter`:** the PR that embeds fonts (`pdf/font`) made
  `docrs-wasm` a `cdylib` only and added a `wasm` profile (release with fat
  LTO and one codegen unit) that `crates/docrs-wasm/build.sh` builds with.
  `[profile.release]` is unchanged, so CI's release builds of the CLI and
  `docrs-compat` keep their speed. Its measurements are in the last section.
- **Fonts:** `subsetter` with `variable-fonts` for all TrueType subsetting,
  static fonts included.
- **PNG:** decoded in-house on the existing `flate2`. Dimensions must be
  checked against `Limits` before allocating, and the decoder needs a fuzz
  test.

## docrs-wasm itself

This branch's `docrs-wasm` was built for `wasm32-unknown-unknown`, then run
through `wasm-bindgen --target nodejs`. Sizes are of `docrs_bg.wasm`: raw
bytes, then `gzip -9`.
- **Profiles:** set with `CARGO_PROFILE_RELEASE_*` environment variables.
- **Crate type:** "cdylib" means `cargo rustc --lib --crate-type cdylib`;
  "both" is the `cargo build` that `build.sh` runs.
- **"+ subsetter":** a temporary copy of the crate with `subsetter = "=0.2.6"`
  (variable fonts) and one exported function that calls
  `subset_with_variations`.

| Build | Crate type | Raw | Gzip | `fromHtml` |
| --- | --- | ---: | ---: | ---: |
| today's profile | both | 3,277,966 | 1,290,312 | 62.3 ms |
| today's profile | cdylib | 3,279,252 | 1,290,099 | |
| fat LTO, 1 codegen unit, opt 3 | both (LTO dropped) | 2,883,959 | 1,255,312 | |
| **fat LTO, 1 codegen unit, opt 3** | **cdylib** | **3,106,807** | 1,296,160 | **58.7 ms** (−5.7%) |
| fat LTO, 1 codegen unit, opt s | both (LTO dropped) | 3,272,823 | 1,143,341 | |
| fat LTO, 1 codegen unit, opt s | cdylib | 2,473,656 | 1,097,026 | 72.1 ms (+15.8%) |
| + subsetter, today's profile | both | 4,601,752 | 1,625,434 | |
| **+ subsetter, fat LTO, opt 3** | **cdylib** | **3,796,150** | 1,516,150 | 59.6 ms (−4.2%) |
| + subsetter, fat LTO, opt s | cdylib | 3,171,117 | 1,292,046 | |

`fromHtml` timing:
- **Input:** `tests/fixtures/markdown/tiptap/{core,extras,nested}.html`
  joined and repeated 20 times (109,979 bytes of HTML), converted with
  `examples/markdown/reference.docx` and `toc: true`.
- **Method:** 8 rounds, with the four timed builds in rotating order. Each
  round runs one Node process per build: 3 warm-up calls, then the median of
  15 calls. The table gives the median of the 8 round medians.
- **Spread:** round medians stayed within 2 ms of each other except for one
  slow round per build, caused by the load from other jobs.

For comparison, the reviews of PR #44 measured:
- **Fable**, `showcase.md` × 20: fat LTO at opt 3 was about 15% faster, and
  opt s about 15% slower.
- **Codex**, 87 KB of HTML, 16 paired runs: opt s was 31.9% slower (82.46
  against 108.78 ms), but without LTO, because the crate was still
  `cdylib + rlib`.

The rest of this file uses a scratch `cdylib` probe crate outside the
workspace. It depends on `crates/docrs-wasm` by path and puts each
candidate behind a feature. Being a pure `cdylib`, it gets LTO when the
profile asks for it.

## Candidates, probe sizes

Today's release profile:

| Added to docrs-wasm | Raw | Gzip | Δ raw | Δ gzip |
| --- | ---: | ---: | ---: | ---: |
| nothing (probe baseline) | 3,281,978 | 1,283,844 | | |
| `subsetter` 0.2.6, no default features | 3,401,605 | 1,329,932 | +119,627 | +46,088 |
| `subsetter` 0.2.6, `variable-fonts` | 4,602,280 | 1,615,194 | +1,320,302 | +331,350 |
| `png` 0.18.1, decoder only | 3,368,571 | 1,317,231 | +86,593 | +33,387 |
| `harfrust` 0.16.2, `std` | 4,537,355 | 1,690,392 | +1,255,377 | +406,548 |
| `subsetter` (variable) + `png` | 4,688,347 | 1,648,135 | +1,406,369 | +364,291 |

With fat LTO, `codegen-units = 1` and `opt-level = "s"` (rejected above):

| Added to docrs-wasm | Raw | Gzip | Δ raw | Δ gzip |
| --- | ---: | ---: | ---: | ---: |
| nothing | 2,471,346 | 1,094,872 | | |
| `subsetter`, no default features | 2,562,556 | 1,123,746 | +91,210 | +28,874 |
| `subsetter`, `variable-fonts` | 3,171,119 | 1,290,870 | +699,773 | +195,998 |
| `png`, decoder only | 2,551,133 | 1,126,171 | +79,787 | +31,299 |

At fat LTO and opt 3, `subsetter` with variable fonts adds 689,343 bytes to
the real crate: 3,106,807 → 3,796,150, from the table above.

## Crates

| Crate | Licence | MSRV declared | Builds with 1.88 | Adds (normal deps, wasm32) | `unsafe` |
| --- | --- | --- | --- | --- | --- |
| `subsetter` 0.2.6 static | MIT OR Apache-2.0 | 1.85 | yes | `rustc-hash` | `deny(unsafe_code)`; none in `rustc-hash` |
| `subsetter` 0.2.6 `variable-fonts` | MIT OR Apache-2.0 | 1.85 | yes | `skrifa` 0.42, `read-fonts` 0.39, `write-fonts` 0.48, `font-types` 0.11, `kurbo` 0.13, `polycool`, `bytemuck`, `indexmap`, `hashbrown` 0.17, `equivalent`, `smallvec`, `arrayvec`, `rustc-hash`, `log`; build time: `bytemuck_derive` (`syn`) | the font crates are `forbid`/none; `bytemuck`, `hashbrown`, `smallvec`, `arrayvec`, `indexmap` have the usual container `unsafe`; `font-types` has 3 uses |
| `png` 0.18.1 | MIT OR Apache-2.0 | 1.73 | yes | `fdeflate`, `flate2` (shared), `miniz_oxide` 0.9 next to the workspace's 0.8, `adler2`, `crc32fast` (shared), `simd-adler32`, `bitflags` | `png`, `fdeflate`, `miniz_oxide` forbid it; `simd-adler32` has 36 uses |
| `harfrust` 0.16.2 | MIT | 1.85 | yes | `read-fonts` 0.47 (not the 0.39 `subsetter` uses), `font-types` 0.12, `bytemuck`, `smallvec`, `core_maths`, `libm`, `spin`, `yoke`, `zerofrom`, `bitflags`, `stable_deref_trait` (+ `foldhash` with `std`) | `harfrust` forbids it; `spin`, `yoke` have some |

- **`harfrust`:** with `default-features = false` alone it does not build. It
  needs `std` or `libm`.
- **NOTICE.md:** adopting `subsetter` needs entries for its licences.

## Variable font instancing with `subsetter`

The function is `subsetter::subset_with_variations(data, 0,
&[(Tag::new(b"wght"), w)], &remapper)`. It was run on Inter
(`Inter[opsz,wght].ttf`, 876,576 bytes) and Roboto Flex (13 axes,
1,787,292 bytes) from google/fonts, at weights 400 and 700, keeping
printable ASCII and `–—‘’“”…é`, 103 code points:

- **Output tables:** `glyf head hhea hmtx loca maxp name post`, plus `prep`
  at 400. There is no `fvar`, `gvar`, `HVAR`, `avar`, `STAT` or `cmap`.
  That is a static TrueType program fit for `FontFile2`, and CoreText loads
  all four outputs.
- **Accuracy:** 103/103 of the output's `hmtx` advances equal skrifa's
  advances for the source at that location (maximum difference 0.00 units),
  and 103/103 outlines are identical point for point.
- **Glyph ids are renumbered:** the remapper's order, plus composite
  components (104 requested, 107–109 out). The PDF writer has to use the
  new ids.
- **Cost:** 0.03–0.6 ms per call natively. The output is 10.7–11.3 KB.
- **Measuring:** `Face::parse` reads only a variable font's default
  instance. At 700 its advances are off by up to 282 units (Inter) and 244
  (Roboto Flex) per 2048. Measuring a variable font at a weight needs the
  instanced advances: wave 3 (the painter) closes this with one source of advances: layout's `FontBook` gives a face per (file, face, `Style`) whose advances are that instance's, chosen by the same Style-to-axis rule the PDF uses (`layout::variation::Fvar::variation`, which `FaceInfo::instance` calls), and a check that each CID's `/W` equals layout's advance for its glyph.

## With font embedding (`pdf/font`)

Measured on 2026-11-27 when `subsetter` 0.2.6 (variable fonts) became a
dependency of the `pdf` feature, on the same machine (Rust 1.97.1,
`wasm-bindgen` 0.2.129, Node 26.13.1, load averages 9 to 12). Sizes are of
`docrs_bg.wasm` from `crates/docrs-wasm/build.sh`; "no name" is the same
file without its `name` section.

| Build | Raw | Gzip | Raw, no name | Gzip, no name |
| --- | ---: | ---: | ---: | ---: |
| `pdf/engine` before (today's profile, `cdylib + rlib`) | 3,360,100 | 1,315,573 | 2,811,690 | 1,257,787 |
| this branch (fat LTO, 1 codegen unit, opt 3, `cdylib`) | 3,179,627 | 1,317,611 | 2,889,964 | 1,281,874 |
| the same, `wasm` profile, on #48 (re-measured) | 3,184,234 | 1,319,191 | 2,893,578 | 1,283,214 |
| this branch + `pdf` + `test-support`, nothing calling them | 3,181,209 | 1,318,086 | | |
| this branch + a probe export that embeds a font | **3,982,595** | 1,575,260 | 3,598,408 | 1,527,545 |

- **Shipped package:** `docrs-wasm` does not enable `pdf` yet, so the
  published wasm is the second row, 180 KB smaller than before. The third
  row is the same build from the `wasm` profile after rebasing on #48
  (FontSet); the 4.6 KB is #48's code.
- **Budget:** once the PDF export reaches the npm package, the font path
  costs about 801 KB raw (the probe against the `pdf` + `test-support` row: `subsetter`,
  `skrifa`, `write-fonts`, the PDF writer and `pdf/font`), which puts the
  wasm at 3.98 MB, over the 3.9 MB budget before the painter and PNG
  decoder. Without the `name` section it is 3.60 MB. The probe export
  calls `text_document_for_tests` (`test-support`), which writes a one-page
  PDF with a line of text in the given font.
- **`fromHtml`:** this branch against `pdf/engine`, same input and method
  as above (3 warm-up calls, median of 15, 8 rounds alternating which build
  runs first). Medians of the round medians: 66.7 ms before, 63.2 ms after
  (−5.2%). Round medians, before: 63.2 63.4 64.9 65.3 68.1 69.9 70.4 76.4;
  after: 59.8 60.6 60.9 62.3 64.1 64.5 76.8 108.3 (the first round ran
  under a load spike).

### The new dependency tree

`cargo tree -p docrs --features pdf` adds, with `subsetter`: `skrifa`
0.42.1, `read-fonts` 0.39.2, `write-fonts` 0.48.1, `font-types` 0.11.3,
`kurbo` 0.13.1, `rustc-hash` 2.1.3, `bytemuck` (+ derive), `smallvec`,
`arrayvec`, `indexmap`, `hashbrown`, `equivalent`, `log` and `polycool`.

- **Licences:** all MIT OR Apache-2.0; `bytemuck` also offers Zlib.
- **MSRV:** the highest is 1.85; `cargo +1.88.0 check --workspace
  --all-targets --features docrs/pdf` passes.
- **Unsafe code:** `subsetter`, `skrifa` and `read-fonts` deny or forbid it.
  `write-fonts`, `kurbo`, `polycool` and `rustc-hash` have none. `font-types`
  has 3 uses; `bytemuck`, `smallvec`, `arrayvec`, `indexmap`, `hashbrown`
  and `log` are the usual collection and casting crates with their own.
- **wasm32:** `cargo check -p docrs --features pdf --target
  wasm32-unknown-unknown` passes, as CI runs it.
