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
`version()`. Errors are thrown as `Error`s with `code`, `reason`, `refusal`,
`limit` and `details`; a refusal produces nothing.

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
measured, and a PDF drawn, with them:

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
`inspectReference` and `markdownStyles` take no `fonts`.

Budgets, by `limits`. Sizes are checked before anything is copied, and
every call checks the set again against its own `limits`, so a set loaded
under the default budgets is refused by a `serverless` call it exceeds.
Live sets are the handles not yet freed or collected, counted across
instances: a reset frees no room.

| | default | serverless |
| -- | -- | -- |
| one file / the set | 32 / 64 MiB | 8 / 16 MiB |
| faces in a set | 64 | 24 |
| live sets | 64 | 8 |
| bytes of all live sets | 4 GiB | 128 MiB |

`loadFontManifest(entries, options?)` builds a set from entries `{ id,
family, bold?, italic?, url | data }`. Each entry's file must hold its
`family` in the declared style (`font_face_mismatch` otherwise; a variable
face covers both weights). `data` is copied. A `url` must be a string and
https, and is fetched without its fragment and with `redirect: "error"`, so
a redirect fails the fetch (`font_fetch_failed`); a response that was
redirected anyway, by a `fetch` of your own, is refused
(`font_url_redirected`). Https alone does not stop a request to a host
you did not mean, so give only your own storage's URLs (signed URLs, for
example), never a URL a tenant typed. Each file and all of them together
are capped by the budgets above and by `maxBytes`, while they stream (a
response without a stream is read whole and then capped, before anything
is copied into WebAssembly memory). `timeoutMs` (1 to 2,147,483,647,
default 30,000) bounds the fetches together and `signal` aborts them.
Every entry is fetched; errors name the entry's `id`, `family` and style.
Pass `fetch` to use your own.

Nothing is cached unless you pass a `cache` from `createFontCache({
maxBytes? })` (64 MiB by default, least recently used dropped first). It
keeps fetched files by entry `id`, never by URL, so an id must name one
file: make one cache per tenant, and give each file an id that changes with
its content (a tenant id and a content hash, for example).

## PDF (experimental)

Every conversion also draws its document as a PDF with `pdf`, and `toPdf`
draws a `.docx` as `docrs pdf` does:

```js
const { fromHtml, toPdf } = require("docrs-wasm");

const { docx, report, pdf, pdfReport } = fromHtml(html, { reference, fonts, pdf: true });
const { pdf: only } = fromHtml(html, { reference, fonts, pdf: { strict: true }, docx: false });
const { pdf: again } = toPdf(docx, { fonts, limits: "serverless" });
// pdfReport: { schema_version: 1, pages, warnings: [{ code, subject, message, locations, count }] }
```

Without `pdf` a conversion's result and types are unchanged. `docx: false`
leaves the `.docx` out (and needs `pdf`). `pdf` takes `strict` (refuse with
`pdf_not_faithful` when the PDF would leave anything out or change it;
default: the conversion's `strict`) and the caps `maxPages`,
`maxOutputBytes`, `maxImagePixels` and `maxDocumentPixels`; `toPdf` takes
them as options of its own. A cap only ever lowers the `limits` profile's:

| | default | serverless |
| -- | -- | -- |
| pages | 2,000 | 500 |
| output bytes | 256 MiB | 32 MiB |
| decoded pixels per picture / per document | 32 M / 256 M | 4 M / 32 M |

Each character is drawn in the face that measured it: a font the document
embeds, else one of `fonts`. A family and style with neither refuses with
`pdf_font_program_missing`, and `error.details.missing_fonts` lists each `{
family, bold, italic }`. While the engine is incomplete, a construct it
does not draw yet refuses with `pdf_not_yet_drawn` (`error.details.warnings`
counts them), so a PDF never silently leaves something out.

`usedFonts` lists the faces a PDF of a document needs and its template does
not embed, so a server fetches only those from its font storage:

```js
usedFonts(docx); // [{ family: "Inter", bold: false, italic: false }, { family: "Inter", bold: true, ... }]
usedFonts({ html, reference, styles }); // or { markdown } or { json }, with the conversion's options
```

A call that draws a PDF leaves the instance's memory where its peak was,
like any other: the package replaces an instance past 128 MiB after the
call and loads font sets into the next one when a call first uses them.
Measured in Node 26: 130 pages of editor HTML (the most the `serverless`
source budget allows) with a 24-face set leave 40 MiB; 770 pages under the
default budgets leave 119 MiB.

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

Fonts (experimental): keep one font set per tenant for the life of the
process, and memoise the promise of `loadFontManifest`, not the set, so
concurrent cold requests fetch once and share one handle (each extra handle
counts against the live-set budget):

```ts
import { createFontCache, loadFontManifest, type FontManifestEntry, type FontSet } from "docrs-wasm";

const fontSets = new Map<string, Promise<FontSet>>();

function tenantFonts(tenant: string, entries: FontManifestEntry[]): Promise<FontSet> {
  let fonts = fontSets.get(tenant);
  if (!fonts) {
    fonts = loadFontManifest(entries, { cache: createFontCache(), limits: "serverless" });
    fonts.catch(() => fontSets.delete(tenant)); // retry after a failure
    fontSets.set(tenant, fonts);
  }
  return fonts;
}
// When a tenant's fonts change: (await fontSets.get(tenant))?.free(); fontSets.delete(tenant);
```

A PDF in the tenant's fonts (experimental): the manifest lists every face
the tenant uploaded, `usedFonts` says which ones the document draws, and
only those are fetched (over https, capped in bytes and time, and cached
by `id` across requests). The set is built for the request and freed after
it, so live sets never pile up; the cache keeps the bytes:

```ts
// app/api/pdf/route.ts
import { readFile } from "node:fs/promises";
import { createFontCache, fromHtml, loadFontManifest, usedFonts, type FontCache, type FontManifestEntry, type UsedFont } from "docrs-wasm";
export const runtime = "nodejs";

const reference = new Uint8Array(await readFile("templates/advice.docx"));
const caches = new Map<string, FontCache>();

// Your storage: every face the tenant uploaded, each a signed https URL.
declare function manifestOf(tenant: string): Promise<FontManifestEntry[]>;

// Both weights of a style: a variable file declared regular draws bold too.
const draws = (used: UsedFont[]) => (entry: FontManifestEntry) =>
  used.some((face) => face.family.toLowerCase() === entry.family.toLowerCase() &&
    face.italic === Boolean(entry.italic));

export async function POST(request: Request) {
  const tenant = request.headers.get("x-tenant") ?? "";
  const html = await request.text();
  const used = usedFonts({ html, reference, limits: "serverless" });
  const entries = (await manifestOf(tenant)).filter(draws(used));
  let cache = caches.get(tenant);
  if (!cache) caches.set(tenant, (cache = createFontCache({ maxBytes: 32 << 20 })));
  const fonts = await loadFontManifest(entries, {
    cache, limits: "serverless", timeoutMs: 10_000, signal: request.signal,
  });
  try {
    const { pdf } = fromHtml(html, { reference, fonts, pdf: true, docx: false, limits: "serverless", redact: true });
    return new Response(pdf.slice(), {
      headers: { "Content-Type": "application/pdf", "Content-Disposition": 'attachment; filename="advice.pdf"' },
    });
  } catch (error: any) {
    if (error.reason !== "pdf_font_program_missing") throw error;
    // A face the tenant never uploaded: ask for it by name.
    return Response.json({ missing: error.details.missing_fonts }, { status: 422 });
  } finally {
    fonts.free();
  }
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
