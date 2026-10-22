# docrs-wasm

Markdown or rich-text editor HTML to Word (`.docx`) with a reference
document, compiled to WebAssembly for Node.js. Your template supplies every style, the lists'
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
(WebAssembly has no unwinding here): the call throws
`RuntimeError: unreachable` and the panic message goes to `console.error`.
The instance may still answer calls afterwards, but its memory can be
inconsistent, so load the module again before the next call (for example
`delete require.cache[require.resolve("docrs-wasm")]` and `require` it
again; existing references keep the old instance).

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

MIT licensed; portions adapted from paper-docx (see `NOTICE.md`).
