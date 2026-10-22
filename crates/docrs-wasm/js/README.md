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
cell alignment, column widths, merged cells (`colspan`/`rowspan`), picture
sizes and alignment, and page breaks. CSS it cannot map is reported
(`html_style_dropped`).

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

A panic is a bug in docrs-wasm. The module is built to abort on a panic
(WebAssembly has no unwinding here): the call throws a
`WebAssembly.RuntimeError` (`unreachable`; after repeated panics `memory
access out of bounds`) with no `code`, and the first panic's message goes
to `console.error`. The instance may still answer calls afterwards, but
every panic leaks what the call had allocated and a later panic prints
nothing, so treat any `RuntimeError` as the end of that instance: load the
module again before the next call (`delete
require.cache[require.resolve("docrs-wasm")]` and `require` it again, or
`createRequire` from an ES module; `import()` returns the cached module)
and retry once. Existing references keep the old instance alive; drop them
and its memory is collected.

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
it to `fromJson`. On a long-lived server (one process serving many
exports) load the module through a small wrapper that reloads it after a
panic, as described above, and retries once:

```ts
// lib/docrs.ts
import { createRequire } from "node:module";

type Docrs = typeof import("docrs-wasm");
const require = createRequire(import.meta.url);
let docrs: Docrs = require("docrs-wasm");

/** Run `use` against the module; after a panic (a WebAssembly.RuntimeError,
 *  never a docrs error with a `code`) load a fresh instance and try once more. */
export function withDocrs<T>(use: (lib: Docrs) => T): T {
  try {
    return use(docrs);
  } catch (error) {
    if (!(error instanceof WebAssembly.RuntimeError)) throw error;
    delete require.cache[require.resolve("docrs-wasm")];
    docrs = require("docrs-wasm");
    return use(docrs);
  }
}
```

```ts
// app/api/export/route.ts
import { readFile } from "node:fs/promises";
import { withDocrs } from "@/lib/docrs";
export const runtime = "nodejs";

const reference = new Uint8Array(await readFile("templates/letter.docx"));

export async function POST(request: Request) {
  const document = await request.json(); // editor.getJSON() from the client
  const { docx, report } = withDocrs((lib) =>
    lib.fromJson(document, { reference, limits: "serverless", redact: true }),
  );
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
