# Public capability ledger

Every public capability of the Rust port has one row. A row is **shipped**
only when a shared behavioral case or integration test proves the reopened
result (or the unchanged state for refusals). Status vocabulary:

* `shipped` — implemented and covered by named tests;
* `refuses` — the operation exists but deliberately refuses this structure
  with a typed refusal (also test-covered);
* `planned(Mx)` — not yet implemented; calling code cannot reach it;
* `out-of-scope` — excluded by design.

Unsupported operations never report success as a no-op.

| Capability | Rust surface | Status | Evidence |
| --- | --- | --- | --- |
| Guarded ZIP/OPC open | `Document::open*`, `package::zip_read::read_archive` | shipped (M1) | `tests/zip_guard.rs`, `kernel-parity` hostile corpus |
| Content types / relationship graph validation | `Package::open` | shipped (M1) | `tests/zip_guard.rs::relationship_graph_defects`, parity |
| Blank document | `Document::new` | shipped (M1) | `tests/kernel.rs::new_document_opens_bundled_template` |
| Lossless XML arena (scopes, unknown markup, comments/PIs, UTF-16 input) | `xml::{parse, serialize, XmlTree}` | shipped (M1) | `tests/xml_preservation.rs`, `kernel.rs::every_xml_part_round_trips_semantically` |
| Semantic XML equivalence | `xml::trees_equivalent`, `package::patch::xml_equivalent` | shipped (M1, QName correction) | `xml_preservation.rs`, parity diffs |
| Package diff | `package::diff::diff_packages`, CLI `diff` | shipped (M1) | parity: 78 fixture diffs match oracle |
| Patch save (no-op byte identity, restore-if-equivalent) | `Document::patch_bytes`, `Document::save`, CLI `roundtrip` | shipped (M1) | `kernel.rs`, `atomic_save.rs`, parity no-op saves |
| Atomic filesystem save (temp+fsync+rename, symlink, mode) | `package::save::write_atomic` | shipped (M1; CI on Linux, macOS, Windows; mode/symlink tests Unix-only) | `atomic_save.rs` incl. failure injection |
| Diagnosis | `package::diagnose::{diagnose_path, diagnose_bytes}`, CLI `diagnose` | shipped (M1) | parity: all kinds match oracle |
| Resource budgets | `Limits`; compare sequence-matching budget | shipped (package/XML parse M1; compare budget M6 refuses `unsupported_structure(sequence_budget)` as upstream) | `zip_guard.rs::budgets_are_resource_limits_not_malformed`, `m6-parity` compare refusals |
| Stories, views (current/original/all), blocks, outline, blind regions | `Document::{stories, blocks, outline}`, CLI `outline` | shipped (M2) | `m2-parity`: 123/123 outlines identical to oracle; `tests/search.rs` |
| Exact / normalized search with Unicode source maps, `nth`, `near` | `Document::{find_text, find_one}`, CLI `find` | shipped (M2) | `m2-parity`: 6,412 queries / 9,979 spans identical; `tests/search.rs` |
| Opaque span handles with local freshness evidence | `Span` | shipped (M2/M3) | `tests/replace.rs` freshness/alias/foreign tests |
| Formatting inspection (`format_of`, style resolution) | `formatting::format_of` | shipped (M4) | ops `m4-formatting-*`: 110 pass, 2 deviation |
| Styles read API (names, ids, defaults, latent styles) | `styles::Styles` | shipped (M4) | ops `m4-styles-*`: 56 pass, 1 deviation |
| Protection status / acknowledgment / gate | `Document::{protection_status, acknowledge_protection}`; every writer, inherited authoring, sections, styles and core properties included | shipped (M3; inherited writers gated in 0.2.0) | `tests/replace.rs::protection_gate_follows_the_matrix`, `tests/protection_gate.rs`, `tests/cases/m3-protection.json` |
| Safe untracked replacement (same owner, affix-localized, narrowing across breaks) | `Document::replace`, CLI `replace` | shipped (M3) | `m3-parity`: 2,565 cases identical + 1 deviation; `tests/replace.rs`; CLI tests |
| replace_all (atomic batch, per-match refusals) | `Document::replace_all`, CLI `replace --all` (exit 3 and no output when nothing is replaced) | shipped (M3) | `m3-parity`, `tests/replace.rs::replace_all_reports_refusals_and_commits_the_rest_atomically`, `cli.rs::replace_all_reports_the_count_and_refuses_when_nothing_matches` |
| `preserve_revision` editing of insertions | `ReplaceOptions::preserve_revision` | implemented, not yet covered (no case or test sets it) | — |
| Replacement crossing paragraphs, hyperlinks, controls, fields, revisions, positional markers, bookmarks (hollowing) | `Document::replace` | refuses (typed) | `m3-parity`, `tests/cases/m3-structure-refusals.json` |
| Protection write (`set_protection`) | `Document::set_protection` | shipped (M4) | ops `m4-protection*` |
| Inherited authoring: add_paragraph/heading/page_break/table/picture/section; paragraph/run/table/cell handles | `authoring.rs` | shipped (M4) | ops `m4-authoring-*`: 153 pass, 27 deviation |
| Inherited API completion: block containers, full paragraph format (line-spacing rule), fonts incl. theme colors, tab stops, inline-shape sizes, inner content, page-break fragments, comment ranges, pictures from a path, `Length`/`RGBColor` | `authoring.rs` | shipped (M7) | ops `m7-inherited-*` text areas: 504 pass, 12 deviation; `tests/authoring_more.rs` |
| Tables: table/row/column/cell properties, `Cell::merge`, editing inside cells, nested tables | `authoring.rs` | shipped (M7) | ops `m7-inherited-tables-*`: 212 pass, 6 deviation |
| Sections, headers/footers (link/unlink, definitions), odd/even setting | `sections.rs` | shipped (M7) | ops `m7-inherited-sections/hdrftr/settings-*`: 73 pass, 10 deviation; `tests/sections.rs` |
| Core properties read/write | `coreprops.rs` | shipped (M7) | ops `m7-inherited-coreprops-*`: 42 pass, 8 deviation; `tests/coreprops.rs` |
| Style writing (add/delete, setters, fonts, paragraph format, latent styles) | `styles_write.rs` | shipped (M7) | ops `m7-inherited-styles-*`: 72 pass, 8 deviation; `tests/styles_write.rs` |
| Hyperlink retargeting (`Hyperlink.address` setter) | `authoring::HyperlinkRef::set_address` | shipped (M7) | ops `m7-inherited-hyperlink-address`: 11 pass, 1 deviation |
| `Image.from_file` | `image::Image::from_file` | shipped (M7) | ops `m7-inherited-image-from-file`: 12 pass, 1 deviation |
| Inherited python-docx member matrix | — | 263 shipped, 8 excluded (Python object-model back-references), 0 not done | ops `m7-inherited-*` |
| Image characterization (BMP/GIF/JPEG/PNG/TIFF) | `image::Image::from_blob` | shipped (M4) | ops `m4-image-*`: 48 pass |
| Picture replacement | `drawing.rs` | shipped (M4) | ops `m4-drawing-*`: 30 pass, 3 deviation |
| Content controls (list/get/set value, all types, data binding) | `controls.rs` | shipped (M4) | ops `m4-controls-*`: 180 pass |
| Bookmarks (list/create/delete) | `bookmarks.rs` | shipped (M4) | ops `m4-bookmarks-*`: 82 pass, 1 deviation |
| Hyperlinks (add) | `links.rs` | shipped (M4); retargeting shipped in M7 (below) | ops `m4-links-*`: 38 pass, 1 deviation |
| Footnotes / endnotes (add) | `notes.rs` | shipped (M4) | ops `m4-notes-*`: 30 pass, 1 deviation |
| Fields (page, page count, date, REF, caption, TOC) | `fields.rs` | shipped (M4) | ops `m4-fields-*`: 53 pass |
| Numbering (list/apply/ensure/restart) | `numbering.rs` | shipped (M4) | ops `m4-numbering-*`: 67 pass, 2 deviation |
| Block insertion and tracked paragraph delete/replace | `blocks.rs` | shipped (M4/M5) | ops `m4-blocks-*`: 119 pass, 1 deviation |
| Table ops (find, update cell incl. tracked, insert/delete row) | `tableops.rs` | shipped (M4) | ops `m4-tableops-*`: 179 pass |
| Tracked replacement | `ReplaceOptions::tracked` | shipped (M5) | m3-parity: 1,524 tracked successes identical |
| Revisions enumerate/accept/reject (incl. moves, rows, paragraph marks) | `Document::revisions`, `revision::{Revisions, Revision}` | shipped (M5) | ops `m5-revision-*`: 425 pass, 1 deviation |
| Comments (add, reply, resolve, thread, delete, Span::comment) | `comments.rs`, `commentops.rs` | shipped (M5) | ops `m5-comments-*`: 104 pass |
| Text diff / pending changes | `textdiff.rs`, CLI `text-diff`/`pending` | shipped (M6) | m6-parity |
| Compare redline (verified algebra; materialize; protection gate on the original) | `compare::compare`, CLI `compare` | shipped (M6) | `m6-parity` (compare): 544 redlines + 214 refusals identical, 0 mismatches; `tests/compare.rs` |
| Composition: `insert_blocks_from`, `append_document` (styles, numbering, media, hyperlinks, bookmark/REF reconciliation, letterhead) | `composition.rs`, CLI `append`/`insert-from` | shipped (M7) | ops `m7-composition-*`: 212 pass, 3 deviation; `tests/composition.rs`; CLI tests |
| Review CLI: list / accept / reject revisions | CLI `revisions`, `resolve` | shipped (M7) | `crates/docrs-cli/tests/cli.rs` (accept-all equals `multiround-accepted.docx` text) |
| Markdown → DOCX with a reference document (styles by role, placement, front matter, lists, tables, images, footnotes, links/bookmarks, TOC, LaTeX → OMML) | `markdown::{convert, insert_markdown}` (feature `markdown`), CLI `from-markdown` | shipped (post-M7, not upstream) | `tests/markdown.rs` (74), `tests/markdown_parse.rs` (17: misnested and unclosed inline HTML, front matter indentation, nesting cap, table width, hidden HTML), module unit tests (25), CLI tests (5); exports reopened by python-docx, pandoc and Word (`tools/markdown/check.sh`) |
| Editor HTML → DOCX (paragraphs, headings, lists, tables with spans and widths, colours, fonts, alignment, images, page breaks; unmapped CSS reported) | `markdown::insert_html` (feature `markdown`), CLI `from-markdown --from html` (default for `.html`/`.htm`) | shipped (post-M7, not upstream) | `markdown.rs::editor_html_maps_every_construct`, `tiptap.rs::a_partly_resized_table_keeps_the_widths_the_editor_set`, `html.rs` unit tests, wasm `editor_html_redaction_and_serverless_limits` |
| TipTap JSON → DOCX (`editor.getJSON()` directly: the nodes and marks of Tiptap 3's extensions, plus math, details, mentions, embeds, contents and page breaks the HTML only hints at; unmapped node, mark and attribute types reported) | `markdown::insert_tiptap` (feature `markdown`), CLI `from-markdown --from tiptap` (default for `.json`), WASM `fromJson` (`fromTiptap` kept as an alias) | shipped (post-M7, not upstream) | `tests/tiptap.rs` (13; JSON and HTML of the same editor page render identically, fixtures from Tiptap 3.32, a rule is one plain line from every source), `tiptap.rs` unit tests, wasm `tiptap_json_converts_like_the_editors_html`, CLI `from_markdown_takes_tiptap_json_by_extension_or_flag` |
| Opt-in tolerance for Office `[trash]` members and data-free ZIP directory entries (default: refused, as upstream) | `Tolerance`, `Document::open_bytes_with`/`open_path_with`; CLI reference documents of `from-markdown`, `markdown-styles`, `markdown-inspect` (`--strict-package` refuses) | shipped (post-M7, port extension) | `zip_guard.rs::word_tolerated_irregularities_open_only_on_request`, `markdown.rs::tolerated_reference_packages_convert`, CLI `from_markdown_sets_aside_package_quirks_word_tolerates` |
| Markdown / editor HTML / TipTap JSON → DOCX for Node.js / Next.js (WebAssembly; options, images and template as bytes) | `crates/docrs-wasm`: `fromMarkdown`, `fromHtml`, `fromJson` (alias `fromTiptap`), `inspectReference`, `markdownStyles`, `referenceDocument`, `version` | shipped (post-M7; published to npm by `release.yml` on a version tag) | native tests (10), Node tests `js/test.mjs` (22), local Next.js 16 app via `npm pack` tarball; outputs reopened by python-docx, pandoc and Word |
| Markdown reference document tools | `markdown::{reference_document, role_styles}`, CLI `markdown-reference`, `markdown-styles` | shipped (post-M7) | `starter_reference_has_every_role_style`, CLI `markdown_reference_and_styles_commands` |
| Reference document inspection (role styles, lifted branding, section kept, headers/footers, cover page, compatibility mode, problems) | `markdown::inspect`, CLI `markdown-inspect` | shipped (post-M7) | `markdown.rs::inspection_reports_what_a_conversion_takes`, wasm `inspection_reports_the_section_and_problems` |
| Page layout estimate (Word for Mac line and page breaks; TOC page numbers) | `layout::layout`, CLI `layout` | shipped (post-M7, not upstream) | `tests/layout.rs` (4), `flow.rs` unit tests; TOC accuracy in `tools/layout/README.md` |
| Serverless resource budgets and redacted warnings/errors (wasm: every entry point; sizes checked before inputs are copied into WebAssembly memory) | `Limits::serverless`, `MarkdownReport::redact`, `Error::redacted_message` | shipped (post-M7) | `render.rs::every_warning_code_has_a_redacted_message`, wasm `editor_html_redaction_and_serverless_limits`, `every_entry_point_applies_the_limits`, Node `oversized inputs are refused before they are copied in` |
| Untrusted Markdown/HTML: local images read only from `resource_dir` (none by default; no absolute, UNC, `file:` or escaping paths, symbolic links resolved), pictures bounded by `Limits::max_part_bytes`, damaged pictures skipped with a warning, links limited to `http`, `https`, `mailto`, `tel`, fragments and relative references, template thumbnail left out | `MarkdownOptions::resource_dir`, warnings `image_outside_resource_dir`, `image_too_large`, `image_unreadable`, `link_scheme_skipped` | shipped (0.2.0) | `markdown.rs::local_images_stay_inside_the_resource_directory`, `without_a_resource_directory_no_file_is_read`, `images_larger_than_the_part_limit_are_left_out`, `malformed_images_become_warnings_and_zero_dpi_counts_as_96`, `links_keep_only_web_mail_phone_and_relative_targets`, `reference_styles_and_layout_are_used_unchanged` |
| Rendering, PDF, .doc, encryption, macros, network | — | out-of-scope | by design |
