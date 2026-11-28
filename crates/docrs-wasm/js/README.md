# docrs-wasm

Markdown, rich-text editor HTML or TipTap JSON to Word (`.docx`) with a
reference document, compiled to WebAssembly for Node.js. Your template supplies every style, the lists'
numbering, page setup, headers, footers and branding; the Markdown supplies
the body. No native binaries, no pandoc, no LibreOffice.

```js
const { fromMarkdown } = require("docrs-wasm");

const { docx, report } = fromMarkdown(markdown, {
  reference: templateBytes, // Uint8Array of a .docx (optional)
  toc: true,
  pageNumbers: true,
  images: { "chart.png": chartBytes }, // images the Markdown refers to
});
// docx: Uint8Array of the .docx; report: counts, styles used, warnings
```

Editor HTML (TipTap's `getHTML()` and similar) takes the same options:

```js
const { fromHtml } = require("docrs-wasm");

const { docx, report } = fromHtml(editorHtml, {
  reference: templateBytes,
  images: { [signedUrl]: imageBytes }, // keyed by the img src as written
  limits: "serverless", // budgets for a 256 MB function
  redact: true,         // warnings and errors quote nothing from the input
});
```

It keeps the editor's text colours, highlight colours, fonts, paragraph and
cell alignment, column widths (a resized column keeps its width and the
rest share what is left), merged cells (`colspan`/`rowspan`), picture
sizes and alignment, and page breaks. CSS it cannot map is reported
(`html_style_dropped`).

`fields` fills `{{Name}}` merge fields once converted, in the body,
headers, footers and text boxes (Word may split a field across runs):

```js
const { docx, report } = fromHtml(editorHtml, {
  reference: templateBytes,
  fields: { "Client Name": "Jo Smith", "Adviser": "" }, // "" blanks one
});
// report.fields: { filled: 2, left: ["Other"], refused: [] }
```

`filled` counts replacements, so a field used twice counts twice. Fields
not named are left as they are, and `{{{Name}}}` fills like `{{Name}}`. A
value is text within one paragraph: one with a line break, tab, `{{` or
`}}` is refused.

A TipTap document converts from its JSON directly, without `getHTML()` in
between, with the same options:

```js
const { fromJson } = require("docrs-wasm");

const { docx, report } = fromJson(editor.getJSON(), { reference: templateBytes });
// also takes the JSON as a string
```

(`fromTiptap`, the name in 0.3.0, is the same function and stays as an
alias.)

It maps the nodes and marks of Tiptap 3's own extensions (headings and
paragraphs with `textAlign`, bold, italic, underline, strike, code,
subscript, superscript, links, highlights, text styles, bullet, ordered and
task lists, quotes, code blocks, rules, hard breaks, images (aligned by
their own `textAlign`), tables with
spans and `colwidth`, inline and block math, details, mentions, YouTube
links, a table of contents and page breaks) to the same document the HTML
gives. Node, mark and attribute types with no Word equivalent are reported
(`tiptap_node_dropped`, `tiptap_mark_dropped`, `tiptap_attribute_dropped`);
their text is kept. A source that is not a `doc` node is refused
(`tiptap_json_invalid`). The node-by-node table is
[docs/tiptap.md](https://github.com/endigo/docrs/blob/main/docs/tiptap.md).

Also exported: `inspectReference(reference, options?)` (what a conversion
will take from a template: role styles, lifted branding, the section kept,
headers and footers, compatibility mode, plain-language problems),
`markdownStyles(reference?, styles?, options?)` (which style each element
takes), `referenceDocument()` (a starter template to restyle in Word) and
`version()`. Errors are thrown as `Error`s with `code`, `reason`, `refusal`
and `limit`; a refusal produces nothing.

`limits` applies to every function that takes options. Inputs over budget
are refused by their size before they are copied into WebAssembly memory:
the source text, the reference document (`max_archive_bytes`: 1 GiB by
default, 8 MiB with `serverless`) and, with `serverless`, 384 KiB of source
and 12 MiB of reference and images together. `now` must be a finite number
of milliseconds from 0 to the end of the year 9999.

A panic is a bug in docrs-wasm, and the package recovers from one by
itself. The module is built to abort on a panic (WebAssembly has no
unwinding here): the call that hit it throws a `WebAssembly.RuntimeError`
(`unreachable`) with no `code`, the panic's message goes to
`console.error`, and the next call runs on a fresh instance of the module
(compiled once, when the package loads). There is nothing to reload and
nothing to retry: the same input would panic again. The instance that
panicked is left to the garbage collector with everything the call had
allocated; V8 frees it at its next full collection. Errors docrs reports
(an `Error` with a `code`) leave the instance in place.

## Fonts (experimental)

Contents page numbers are worked out by measuring the template's fonts: a
font it embeds, else the package's own metrics for common fonts, else a
substitute (listed in `report.toc.missing_fonts`). Load the font files your users upload once,
and pass the set to every conversion so the contents page numbers are
measured with them:

```js
const { loadFonts, loadFontManifest, fromHtml } = require("docrs-wasm");

const brand = loadFonts([interVariable, interBoldItalic], { limits: "serverless" });
brand.faces; // [{ family: "Inter", bold: false, italic: false, variable: true, ... }, ...]
const { docx, report } = fromHtml(html, { reference, toc: true, fonts: brand, limits: "serverless" });
brand.free(); // when the tenant's fonts change
```

`loadFonts` reads TTF, OTF, TTC and OTC files, variable fonts included (the
default instance is measured). One file it cannot read refuses them all,
with the file's index in `error.file`. The set keeps its own copy of the
bytes and outlives the module's instance: after a panic, or after the
package replaces an instance whose memory grew past 128 MiB, it is loaded
again into the next one when a call first uses it. `free()` releases it;
using it afterwards throws `pdf_font_handle_disposed`. A set that is never
freed is released once garbage collected, at best. Sets are never shared
or looked up by family: each conversion uses exactly the set it is given.

Budgets, by `limits` (checked on the sizes before anything is copied, and
again by every call against its own `limits`, so a set loaded under the
default budgets is refused by a `serverless` call it exceeds):

| | default | serverless |
| -- | -- | -- |
| one file / the set | 32 / 64 MiB | 8 / 16 MiB |
| faces in a set | 64 | 24 |
| sets alive in one instance | 64 | 8 |

`loadFontManifest(entries, options?)` builds a set from entries `{ id,
family, bold?, italic?, url | data }`. A `url` must be https (a redirect
away from https is refused too). Each file and all of them together are
capped by the budgets above and by `maxBytes`; `timeoutMs` (default 30,000)
bounds the fetches together and `signal` aborts them. A fetched file is
cached by `id`, never by URL (up to 64 MiB, least recently used first): give
each uploaded font its own id, and a new id when its file changes. Every
entry is fetched; errors name the entry's `id`, `family` and style. Pass
`fetch` to use your own.

## Next.js

Use it from a Route Handler or Server Action on the Node.js runtime and keep
it out of the server bundle so its `.wasm` file is loaded from the package:

```js
// next.config.js
module.exports = { serverExternalPackages: ["docrs-wasm"] };
```

```ts
// app/api/export/route.ts
import { fromMarkdown } from "docrs-wasm";
export const runtime = "nodejs";

export async function POST(request: Request) {
  const form = await request.formData();
  const markdown = String(form.get("markdown") ?? "");
  const template = form.get("template");
  const reference =
    template instanceof File ? new Uint8Array(await template.arrayBuffer()) : undefined;
  const { docx } = fromMarkdown(markdown, { reference });
  return new Response(docx.slice(), {
    headers: {
      "Content-Type":
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
      "Content-Disposition": 'attachment; filename="document.docx"',
    },
  });
}
```

For a TipTap editor, post `editor.getJSON()` as the request body and hand
it to `fromJson`. A long-lived server (one process serving many exports)
needs no wrapper around the module: a panic throws in the request that hit
it, and the next request runs on a fresh instance.

```ts
// app/api/export/route.ts
import { readFile } from "node:fs/promises";
import { fromJson } from "docrs-wasm";
export const runtime = "nodejs";

const reference = new Uint8Array(await readFile("templates/letter.docx"));

export async function POST(request: Request) {
  const document = await request.json(); // editor.getJSON() from the client
  const { docx, report } = fromJson(document, { reference, limits: "serverless", redact: true });
  return new Response(docx.slice(), {
    headers: {
      "Content-Type":
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
      "Content-Disposition": 'attachment; filename="document.docx"',
    },
  });
}
```

MIT licensed; portions adapted from paper-docx (see `NOTICE.md`).
