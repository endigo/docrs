# docrs

Edit Word (`.docx`) documents without disturbing what you didn't touch, and turn Markdown, editor HTML or TipTap JSON into Word documents styled by a template. Pure Rust, with no Python or native dependencies at runtime.

```toml
[dependencies]
docrs = "0.3"                                       # editing
docrs = { version = "0.3", features = ["markdown"] } # + Markdown/HTML to Word
```

## Editing

```rust,no_run
use docrs::{Document, FindOptions, Limits, ReplaceOptions};

let mut document = Document::open_path("in.docx", &Limits::default())?;
let span = document.find_one("payment terms", &FindOptions::default())?;
document.replace(&span, "payment schedule", &ReplaceOptions::default())?;
document.save("out.docx")?;
# Ok::<(), docrs::Error>(())
```

The library also covers search with normalisation, outlines, tracked changes (make, list, accept, reject), comments, comparison into a redline, composing documents with style reconciliation, numbering, tables, sections, fields, bookmarks, links and footnotes.

**Guarantees:**

- **Preservation:** parts are edited in place in a namespace-preserving XML arena. Markup the library doesn't understand survives. A no-op save is the original archive, byte for byte, and a save rewrites only the parts an operation changed.
- **Atomic refusals:** every operation validates first and commits last. A refusal leaves the document, its live handles and any existing output file unchanged.
- **Fresh handles:** a span or block handle used after its text moved, or in another document, is refused instead of editing something else.
- **Unicode:** offsets are Unicode scalar positions, and normalised matches map back to whole source characters.
- **Guarded reads:** ZIP and XML parsing enforce resource limits (`Limits`). Hostile packages are refused, not trusted.

## Markdown and HTML to Word (`markdown` feature)

```rust,no_run
# #[cfg(feature = "markdown")] {
use docrs::markdown::{convert, MarkdownOptions};
use docrs::{Document, Limits};

let template = Document::open_path("template.docx", &Limits::default())?;
let options = MarkdownOptions { toc: Some(true), ..MarkdownOptions::default() };
let (document, report) = convert("# Advice\n\nText.\n", Some(template), &options)?;
document.save("advice.docx")?;
println!("{} warnings", report.warnings.len());
# }
# Ok::<(), docrs::Error>(())
```

**What the template supplies:** every style, the lists' numbering, page setup, headers, footers and branding. Like pandoc's `--reference-doc`, but formatting the template shows only by hand in its sample content is lifted into proper styles.

**Inputs:** CommonMark with GitHub tables, task lists, footnotes and alerts; definition lists, front matter and LaTeX math (as Word equations). `docrs::markdown::insert_html(&mut document, html, &options)` renders editor HTML into a document instead, and `insert_tiptap(&mut document, json, &options)` a TipTap document (`editor.getJSON()`) with its marks, text styles, tables, task lists, math and page breaks intact; nodes, marks and attributes with no Word equivalent are reported (`tiptap_node_dropped`, `tiptap_mark_dropped`, `tiptap_attribute_dropped`).

**Hostile input:** structure nested deeper than `docrs::markdown::MAX_NESTING_DEPTH` (128 levels of quotes, lists, spans, HTML elements or math groups) is refused with `ErrorKind::ResourceLimit` (`markdown_nesting_too_deep`) and nothing changes. HTML browsers do not display (`<script>`, `<style>`, `<head>`, …) is dropped with a warning, and tables are narrowed to Word's 63 columns with a warning.

**Untrusted input:** by default nothing is read from disk; pictures come from `MarkdownOptions::images` and `data:` URIs. Set `resource_dir` to read local images, which must then stay inside that directory (no absolute, UNC or `file:` paths, no `..` or symbolic link out of it). Pictures over the document's `Limits::max_part_bytes` and damaged pictures are left out with a warning, and links keep their target only for `http`, `https`, `mailto`, `tel`, `#fragment` and relative references. Warnings quote the source, never a resolved path.

**Table of contents:** it is written out with page numbers computed by estimating Word for Mac's layout from bundled font metrics, so the document opens without an update prompt. Against Word's own update, 95.8% of 1,344 entries on 84 test documents match exactly, and none is more than one page off.

## License

MIT. Adapted material and the upstream notices are listed in `NOTICE.md`. The bundled font metrics are numbers only; no font software is included.
