# Compatibility with paper-docx

docrs ports the behaviour of the Python [paper-docx](https://github.com/paper-instruments/paper-docx) 0.2.1 (commit `3c0f9a5`). Every operation was checked against that version as a test oracle (`cargo run --release -p docrs-compat -- check --upstream-rev 3c0f9a5`). Where the two differ on purpose, this page says how.

## Intentional differences

| Area | paper-docx | docrs | Evidence |
| --- | --- | --- | --- |
| Unreachable members on save | dropped (graph rebuild) | preserved | parity `orphan-member`, `escaped-space-name`; `kernel.rs::orphan_members_survive_a_changed_save` |
| Member case differs from relationship | member renamed on save | original name kept | parity `rel-case-mismatch-target` |
| DOCTYPE in story parts | accepted by oxml parser | refused (`xml_dtd_prohibited`) | parity `document-internal-dtd` |
| XML depth > 256 | lxml `XMLSyntaxError` | `resource_limit(max_xml_depth)` | parity `xml-depth-257` |
| Raw exceptions on open (BadZipFile, KeyError, XMLSyntaxError, NotImplementedError) | leak as non-refusals | `malformed_package` with stable reason | 9 `classified` parity rows |
| QName-valued attribute values in equivalence | textual | resolved (`xsi:type`, `mc:*`, `Choice@Requires`) | `xml_preservation.rs::qname_valued_attributes_compare_by_resolved_namespace` |
| Non-Word main part (`.dotx` etc.) | `ValueError` | `invalid_argument(not_a_word_document)` | `zip_guard.rs::main_part_kind_and_xml_validity` |
| Editing spans captured under `view=original`/`all` | allowed when no revision is involved | refused (`historical_view_target`) | `replace.rs::historical_view_targets_are_inspection_only` |
| Detach-and-reinsert of a text node | span stays valid (same lxml identity) | span stale (structural generation) | `Signature::struct_gen` in `src/search.rs` (no dedicated test) |
| Unknown story root element | `ValueError` crash | `unsupported_structure(unrecognized_story_root)` | `src/story.rs` (no dedicated test) |
| XML encodings | any libxml2/iconv encoding | UTF-8, UTF-16, ASCII, ISO-8859-1, windows-1252; others refused | `xml/decode.rs` tests |
| Core property dates before year 1000 | written with `strftime("%Y")`: `0999` on macOS, `999` on Linux (then read back as `None`) | always four digits (`0999`) | ops `m7-inherited-coreprops-*` `dates-each` |
| Office `[trash]` members, empty-stream directory entries | refused | refused by default; set aside with opt-in `Tolerance` (CLI: reference documents of `from-markdown`, `markdown-styles` and `markdown-inspect`; `--strict-package` to refuse) | `zip_guard.rs::word_tolerated_irregularities_open_only_on_request` |

## Known limitations

- **Block-level placeholder controls:** paper-docx only records `sdt` on atoms for inline controls. So for a block-level control showing placeholder text, a partial replacement is allowed and `w:showingPlcHdr` / `PlaceholderText` styling are not cleared. docrs keeps this behaviour for parity (`tests/cases/m3-structure-refusals.json`).
- **`diagnose`** mirrors paper-docx's shallow check: it reports `broken-rel.docx` as readable while `open` refuses it.
- **Markdown:** LaTeX outside the supported subset, SVG images and HTML outside the supported subset (unknown tags, reported by name; their text is kept) are not converted. Remote images are never fetched. HTML browsers do not display (`head`, `title`, `style`, `script`, `template`, `noscript`, …) is dropped with its content (`html_element_skipped`). Tables are at most 63 columns, Word's limit: wider spans are cut back and cells past the last column are merged into it (`table_columns_clamped`). Sources nested deeper than 128 levels (`MAX_NESTING_DEPTH`) are refused (`resource_limit`, `markdown_nesting_too_deep`).
- **Table of contents page numbers** estimate Word for Mac's layout. Word for Windows uses other font metrics, so its numbers can differ for some fonts (Aptos, Times New Roman). Floating objects, footnotes, columns and multiple sections with different page setups are not modelled.
- **Platforms:** CI tests on Linux, macOS and Windows, and runs the independent LibreOffice loader check on Linux.
