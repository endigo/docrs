// Node tests of the built package (crates/docrs-wasm/pkg):
//   node --test crates/docrs-wasm/js/test.mjs
// Fixtures are the synthetic templates and sample Markdown in
// tests/fixtures/markdown/templates.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { test } from "node:test";
import timers from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import v8 from "node:v8";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const pkg = process.env.DOCRS_WASM_PKG ?? join(here, "..", "pkg");
const require = createRequire(import.meta.url);
const lib = require(pkg);
const freshSize = lib.__wasm.memory.buffer.byteLength;
const templates = join(root, "tests", "fixtures", "markdown", "templates");
const readBytes = (path) => new Uint8Array(readFileSync(path));

const isZip = (bytes) => bytes[0] === 0x50 && bytes[1] === 0x4b;

// A 3×2 PNG at 150 dpi.
const PNG = Uint8Array.from(
  Buffer.from(
    "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082",
    "hex",
  ),
);

test("every synthetic template converts the sample record of advice", () => {
  const markdown = readFileSync(join(templates, "record-of-advice.md"), "utf8");
  const names = readdirSync(join(templates, "v2")).filter((n) => n.endsWith(".docx"));
  assert.equal(names.length, 20);
  for (const name of names) {
    const reference = readBytes(join(templates, "v2", name));
    const { docx, report } = lib.fromMarkdown(markdown, {
      reference,
      toc: true,
      pageNumbers: true,
    });
    assert.ok(docx instanceof Uint8Array && isZip(docx), name);
    assert.deepEqual(report.warnings, [], name);
    assert.ok(report.counts.tables > 0, name);
    assert.equal(lib.markdownStyles(docx).length, 41, name);
  }
});

test("options are optional and the result is deterministic for a given time", () => {
  const first = lib.fromMarkdown("# Title\n\nText.\n", { now: 1_700_000_000_000 });
  const second = lib.fromMarkdown("# Title\n\nText.\n", { now: 1_700_000_000_000 });
  assert.deepEqual(first.docx, second.docx);
  assert.ok(isZip(lib.fromMarkdown("plain").docx));
  assert.ok(isZip(lib.fromMarkdown("plain", { toc: undefined, pageNumbers: null, reference: undefined }).docx));
});

test("images come from memory", () => {
  const { report } = lib.fromMarkdown("![Chart](./img/chart.png)\n", {
    images: { "img/chart.png": PNG },
  });
  assert.equal(report.counts.images, 1);
  assert.deepEqual(report.warnings, []);
  const byUrl = lib.fromMarkdown("![Chart](https://cdn.example/a/chart.png?v=2)\n", {
    images: { "https://cdn.example/a/chart.png?v=2": PNG },
  }).report;
  assert.equal(byUrl.counts.images, 1);
  // A key without an extension still gives the part one (Word refuses
  // a media part without a content type).
  const { docx } = lib.fromMarkdown("![Chart](img-123)\n", { images: { "img-123": PNG } });
  assert.ok(Buffer.from(docx).includes("word/media/image1.png"));
  assert.ok(!Buffer.from(docx).includes("word/media/image1.\u0000"));
  assert.deepEqual(byUrl.warnings, []);
  const missing = lib.fromMarkdown("![Chart](other.png)\n").report;
  assert.equal(missing.warnings[0].code, "image_not_found");
});

test("refusals throw structured errors", () => {
  assert.throws(
    () => lib.fromMarkdown("![x](missing.png)\n", { strict: true }),
    (err) =>
      err instanceof Error &&
      err.refusal === true &&
      err.reason === "markdown_not_faithful" &&
      err.limit === null &&
      /missing\.png was not supplied/.test(err.message),
  );
  assert.throws(
    () => lib.fromMarkdown("x", { nonsense: true }),
    (err) => err.code === "invalid_argument" && err.reason === "invalid_options",
  );
  assert.throws(
    () => lib.fromMarkdown("x", { pagenumbers: true }),
    (err) => /unknown option "pagenumbers"/.test(err.message),
  );
  assert.throws(
    () => lib.fromMarkdown("x", { styles: { not_a_role: "Normal" } }),
    (err) => err.code === "invalid_argument",
  );
});

test("reference packages Word tolerates are read unless strictPackage", () => {
  const reference = readBytes(join(templates, "docx-template-missing-pandoc-styles.docx"));
  const { referenceSetAside } = lib.fromMarkdown("# Advice\n", { reference });
  assert.equal(referenceSetAside.length, 2);
  assert.throws(
    () => lib.fromMarkdown("# Advice\n", { reference, strictPackage: true }),
    (err) => err.reason === "zip_directory_data",
  );
});

test("the starter reference document round-trips", () => {
  const reference = lib.referenceDocument();
  assert.ok(isZip(reference));
  const roles = lib.markdownStyles(reference);
  assert.ok(roles.every((role) => role.style !== null));
  const { report } = lib.fromMarkdown("```\ncode\n```\n", { reference });
  assert.equal(report.styles.code_block.name, "Source Code");
  assert.match(lib.version(), /^\d+\.\d+\.\d+/);
});

const LOGO = { "https://store.example/logo.png?token=t": PNG };
// A source one byte over the serverless budget (384 KiB).
const OVER_BUDGET = "x".repeat((384 << 10) + 1);

// The editor HTML contract (the shapes a rich-text editor emits), synthetic.
const EDITOR_HTML =
  '<h2 style="text-align: center;">Advice</h2>' +
  '<p><span style="color: #dc2626;">red</span> <span style="font-family: &quot;Courier New&quot;, monospace;">mono</span> ' +
  '<mark data-color="#bbf7d0" style="background-color: #bbf7d0; color: inherit;">green</mark></p><p></p>' +
  '<table style="width: 520px;"><colgroup><col style="width: 240px;"><col style="width: 140px;"><col style="width: 140px;"></colgroup><tbody>' +
  '<tr><th colwidth="240"><p>Product</p></th><th colspan="2" colwidth="140,140"><p>Both</p></th></tr>' +
  '<tr><td rowspan="2" colwidth="240"><p>Two rows</p></td><td colwidth="140"><p style="text-align: right;">$1</p></td><td colwidth="140"><p>x</p></td></tr>' +
  '<tr><td colwidth="140"><p>y</p></td><td colwidth="140"><p>z</p></td></tr></tbody></table>' +
  '<div data-image-align="right" style="text-align: right;"><img src="https://store.example/logo.png?token=t" alt="Logo" width="200" height="80"></div>' +
  '<div data-page-break="" class="rich-text-page-break not-prose"></div><h2>After</h2>';

const documentXml = (docx) => {
  const { inflateRawSync } = require("node:zlib");
  const buf = Buffer.from(docx);
  let at = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  const entries = buf.readUInt16LE(at + 10);
  let p = buf.readUInt32LE(at + 16);
  for (let i = 0; i < entries; i++) {
    const method = buf.readUInt16LE(p + 10);
    const size = buf.readUInt32LE(p + 20);
    const nameLen = buf.readUInt16LE(p + 28);
    const extra = buf.readUInt16LE(p + 30);
    const comment = buf.readUInt16LE(p + 32);
    const local = buf.readUInt32LE(p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);
    if (name === "word/document.xml") {
      const start = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28);
      const data = buf.subarray(start, start + size);
      return (method === 8 ? inflateRawSync(data) : data).toString("utf8");
    }
    p += 46 + nameLen + extra + comment;
  }
  throw new Error("no word/document.xml");
};

test("editor HTML maps colours, alignment, spans, widths, images and page breaks", () => {
  const { docx, report } = lib.fromHtml(EDITOR_HTML, { images: LOGO });
  assert.deepEqual(report.warnings, []);
  assert.equal(report.counts.images, 1);
  const xml = documentXml(docx);
  for (const needle of [
    '<w:color w:val="DC2626"/>',
    'w:ascii="Courier New"',
    'w:fill="BBF7D0"',
    '<w:jc w:val="center"/>',
    '<w:gridSpan w:val="2"/>',
    '<w:vMerge w:val="restart"/>',
    '<w:vMerge/>',
    '<wp:extent cx="1905000" cy="762000"/>',
    'descr="Logo"',
    '<w:br w:type="page"/>',
  ]) {
    assert.ok(xml.includes(needle), needle);
  }
  assert.ok(!xml.includes(">Logo<"));
});

test("redact keeps input text out of warnings and errors", () => {
  const { report } = lib.fromHtml('<p><img src="https://x.example/client-name.png"></p><p style="line-height: 2">t</p>', {
    redact: true,
  });
  const text = JSON.stringify(report.warnings);
  assert.ok(text.includes("remote_image_skipped") && text.includes("html_style_dropped"), text);
  assert.ok(!text.includes("client-name") && !text.includes("line-height"), text);
  assert.throws(
    () => lib.fromMarkdown("![x](client-secret.png)\n", { strict: true, redact: true }),
    (err) => err.code === "unsupported_structure" && !err.message.includes("client-secret"),
  );
});

test("the serverless preset bounds input and the reference", () => {
  assert.throws(
    () => lib.fromMarkdown(OVER_BUDGET, { limits: "serverless" }),
    (err) => err.code === "resource_limit" && err.limit.limit === "max_source_bytes",
  );
  assert.ok(isZip(lib.fromMarkdown(OVER_BUDGET.slice(0, 1000), { limits: "serverless" }).docx));
  assert.throws(
    () => lib.fromMarkdown("a", { limits: "serverless", images: { a: new Uint8Array((12 << 20) + 1) } }),
    (err) => err.code === "resource_limit" && err.limit.limit === "max_input_bytes",
  );
  assert.throws(() => lib.fromMarkdown("a", { limits: "huge" }), (err) => err.code === "invalid_argument");
});

test("inspectReference reports the section, lifted branding and problems", () => {
  for (const name of readdirSync(join(templates, "v2")).filter((n) => n.endsWith(".docx"))) {
    const inspection = lib.inspectReference(readBytes(join(templates, "v2", name)), { pageNumbers: true });
    assert.equal(inspection.schema_version, 1, name);
    assert.equal(inspection.section.used, "last", name);
    assert.ok(inspection.section.text_width > 0, name);
    assert.ok(Array.isArray(inspection.problems), name);
  }
  const multi = lib.inspectReference(readBytes(join(templates, "v2", "multi-section-portrait-then-landscape.docx")));
  assert.equal(multi.section.page.orientation, "landscape");
  assert.ok(multi.problems.some((p) => p.code === "multiple_sections"));
  const legacy = lib.inspectReference(readBytes(join(templates, "v2", "compat-word2003-legacy.docx")));
  assert.ok(legacy.problems.some((p) => p.code === "old_compatibility_mode"));
});

// The page numbers and entries themselves are proved natively
// (crates/docrs/tests/markdown.rs); here only the report crosses the boundary.
test("the contents report shows computed page numbers unless tocNumbers is false", () => {
  const markdown = "# One\n\nText.\n\n\\newpage\n\n# Two\n\n\\newpage\n\n\\newpage\n\n# Three\n";
  const { report } = lib.fromMarkdown(markdown, { toc: true });
  assert.deepEqual(report.toc, { entries: 3, pages: 5, missing_fonts: [], embedded_fonts: [] });
  assert.equal(lib.fromMarkdown(markdown, { toc: true, tocNumbers: false }).report.toc, null);
});

test("headings without text get no contents entry", () => {
  const { report } = lib.fromHtml("<h2>A</h2><p>x</p><h1></h1><h2>B</h2><p>y</p>", { toc: true });
  assert.equal(report.toc.entries, 2);
});

test("oversized inputs are refused before they are copied in", () => {
  // Zero-filled and untouched, so this costs no resident memory unless the
  // module copies it into WebAssembly memory.
  const big = new Uint8Array(512 << 20);
  const before = process.memoryUsage().rss;
  const tooBig = (err) => err.code === "resource_limit" && err.limit.limit === "max_archive_bytes";
  assert.throws(() => lib.fromMarkdown("x", { limits: "serverless", reference: big }), tooBig);
  assert.throws(() => lib.fromHtml("x", { limits: "serverless", reference: big }), tooBig);
  assert.throws(() => lib.inspectReference(big, { limits: "serverless" }), tooBig);
  assert.throws(() => lib.markdownStyles(big, undefined, { limits: "serverless" }), tooBig);
  assert.throws(
    () => lib.fromMarkdown("x", { limits: "serverless", images: { a: big } }),
    (err) => err.code === "resource_limit" && err.limit.limit === "max_input_bytes",
  );
  assert.ok(process.memoryUsage().rss - before < 256 << 20, "the reference was copied");
  assert.throws(
    () => lib.fromMarkdown(OVER_BUDGET, { limits: "serverless", redact: true }),
    (err) => err.limit.limit === "max_source_bytes",
  );
  assert.throws(() => lib.fromMarkdown(42), (err) => err.code === "invalid_argument" && err.reason === "invalid_source");
  assert.throws(() => lib.fromMarkdown("x", { limits: 1 }), (err) => err.reason === "invalid_options");
});

test("markdownStyles takes the limits and strictPackage options", () => {
  const reference = readBytes(join(templates, "docx-template-missing-pandoc-styles.docx"));
  assert.equal(lib.markdownStyles(reference, undefined, { limits: "serverless" }).length, 41);
  assert.throws(
    () => lib.markdownStyles(reference, undefined, { strictPackage: true }),
    (err) => err.reason === "zip_directory_data",
  );
  const mapped = lib.markdownStyles(undefined, { code_block: "Normal" }, { styles: { code_block: "Title" } });
  assert.equal(mapped.find((role) => role.role === "code_block").style.name, "Normal");
  assert.throws(() => lib.markdownStyles(undefined, undefined, { limits: "huge" }), (err) => err.code === "invalid_argument");
});

test("now must be a time from 1970 to the year 9999 in milliseconds", () => {
  for (const now of [NaN, Infinity, -Infinity, -1, 253402300800000, 1e300]) {
    assert.throws(
      () => lib.fromMarkdown("x", { now }),
      (err) => err instanceof Error && err.code === "invalid_argument" && err.reason === "invalid_options",
      String(now),
    );
  }
  assert.throws(() => lib.fromMarkdown("x", { now: "today" }), (err) => err.reason === "invalid_options");
  assert.ok(isZip(lib.fromMarkdown("x", { now: 0 }).docx));
  assert.ok(isZip(lib.fromMarkdown("x", { now: 253402300799999 }).docx));
});

const tiptap = (name) => readFileSync(join(root, "tests", "fixtures", "markdown", "tiptap", name), "utf8");

test("TipTap JSON renders the same document as the editor's HTML", () => {
  const options = { images: { "https://store.example/logo.png?token=t": PNG, "https://store.example/seal.png?token=s": PNG }, now: 1_700_000_000_000 };
  const fromText = lib.fromJson(tiptap("core.json"), options);
  const fromObject = lib.fromJson(JSON.parse(tiptap("core.json")), options);
  const fromHtml = lib.fromHtml(tiptap("core.html"), options);
  assert.deepEqual(fromText.report.warnings, []);
  assert.deepEqual(fromText.report.counts, fromHtml.report.counts);
  assert.equal(documentXml(fromText.docx), documentXml(fromHtml.docx));
  assert.deepEqual(fromObject.docx, fromText.docx);
  assert.deepEqual(lib.fromTiptap(tiptap("core.json"), options).docx, fromText.docx);
  const xml = documentXml(fromText.docx);
  for (const needle of ['<w:color w:val="DC2626"/>', 'w:fill="BBF7D0"', '<w:highlight w:val="yellow"/>', '<w:jc w:val="both"/>', '<w:gridSpan w:val="2"/>', 'descr="Logo"']) {
    assert.ok(xml.includes(needle), needle);
  }
});

test("TipTap JSON reports what has no Word equivalent and refuses what is not a document", () => {
  const { docx, report } = lib.fromJson(tiptap("extras.json"));
  const xml = documentXml(docx);
  assert.ok(xml.includes("@Jane") && xml.includes(":rocket:") && xml.includes("<m:oMath"), xml);
  assert.deepEqual(report.warnings.map((w) => w.code), ["tiptap_node_dropped"]);
  assert.ok(report.warnings[0].message.includes("emoji"));
  assert.throws(
    () => lib.fromJson('{"type":"paragraph"}'),
    (err) => err.code === "invalid_argument" && err.reason === "tiptap_json_invalid",
  );
  assert.throws(() => lib.fromJson("{not json"), (err) => err.reason === "tiptap_json_invalid");
  assert.throws(() => lib.fromJson(42), (err) => err.reason === "invalid_source");
  assert.throws(
    () => lib.fromJson(OVER_BUDGET, { limits: "serverless" }),
    (err) => err.code === "resource_limit" && err.limit.limit === "max_source_bytes",
  );
});


// A trap the way a panic ends (the allocator asked for more than the address
// space holds), raised inside a call while the conversion reads the options.
const trapping = {
  images: {
    get "a.png"() {
      lib.__wasm.__wbindgen_malloc(0xfffffff0, 1);
      return PNG;
    },
  },
};
const trapInside = () => lib.fromHtml("<p>x</p>", trapping);
const sample = () => lib.fromHtml(EDITOR_HTML, { images: LOGO, now: 1_700_000_000_000 }).docx;
v8.setFlagsFromString("--expose-gc");
const collect = vm.runInNewContext("gc");

test("a trap replaces the instance and the next call runs on a fresh one", () => {
  const baseline = sample();
  const poisoned = lib.__wasm;
  assert.ok(poisoned.memory.buffer.byteLength > freshSize, "the conversion grew the memory");
  assert.throws(trapInside, (err) => err instanceof WebAssembly.RuntimeError && err.code === undefined);
  assert.notEqual(lib.__wasm, poisoned);
  assert.equal(lib.__wasm.memory.buffer.byteLength, freshSize);
  assert.deepEqual(sample(), baseline);
  for (const call of [
    () => lib.fromMarkdown("x", trapping),
    () => lib.fromJson('{"type":"doc"}', trapping),
    () => lib.fromTiptap('{"type":"doc"}', trapping),
    () => lib.inspectReference(new Uint8Array(4), trapping),
    () => lib.markdownStyles(undefined, undefined, trapping),
  ]) {
    const before = lib.__wasm;
    assert.throws(call, WebAssembly.RuntimeError);
    assert.notEqual(lib.__wasm, before);
  }
  const marked = lib.__wasm;
  assert.throws(() => marked.__wbindgen_malloc(0xfffffff0, 1), WebAssembly.RuntimeError);
  assert.notEqual(lib.__wasm, marked);
  assert.deepEqual(sample(), baseline);
});

test("an error docrs reports leaves the instance in place", () => {
  const instance = lib.__wasm;
  assert.throws(() => lib.fromMarkdown("x", { nonsense: true }), (err) => err.code === "invalid_argument");
  assert.throws(() => lib.fromMarkdown("![x](missing.png)\n", { strict: true }), (err) => err.refusal === true);
  assert.throws(() => lib.fromMarkdown(OVER_BUDGET, { limits: "serverless" }), (err) => err.code === "resource_limit");
  assert.throws(() => lib.inspectReference(new Uint8Array([1, 2, 3, 4])), (err) => err.code === "malformed_package");
  assert.equal(lib.__wasm, instance);
});

test("any exception that unwinds through the module replaces the instance", () => {
  const recurse = () => recurse() + 1;
  const boom = new Error("boom");
  for (const [getter, expected] of [
    [recurse, (err) => err instanceof RangeError && /call stack/.test(err.message)],
    [() => { throw boom; }, (err) => err === boom],
  ]) {
    const before = lib.__wasm;
    assert.throws(() => lib.fromHtml("<p>x</p>", { images: { get "a.png"() { return getter(); } } }), expected);
    assert.notEqual(lib.__wasm, before);
  }
  assert.ok(isZip(lib.fromHtml("<p>x</p>").docx));
});

test("a few hundred traps leave memory flat and the replaced instances unreachable", async () => {
  const baseline = sample();
  const replaced = [];
  for (let i = 0; i < 300; i++) {
    replaced.push(new WeakRef(lib.__wasm.memory));
    assert.throws(trapInside, WebAssembly.RuntimeError);
    assert.equal(lib.__wasm.memory.buffer.byteLength, freshSize, `trap ${i}`);
  }
  assert.deepEqual(sample(), baseline);
  // A WeakRef keeps its target to the end of the job that made it.
  await timers.setImmediate();
  collect();
  await timers.setImmediate();
  collect();
  const alive = replaced.filter((ref) => ref.deref() !== undefined).length;
  assert.ok(alive <= 3, `${alive} of ${replaced.length} replaced instances are still reachable`);
});

test("the entry point exports every function the typings declare, to ESM by name too", async () => {
  const declared = [...readFileSync(join(pkg, "docrs.d.ts"), "utf8").matchAll(/^export function (\w+)\(/gm)]
    .map((m) => m[1])
    .sort();
  assert.ok(declared.includes("fromHtml") && declared.includes("fromJson"));
  assert.deepEqual(Object.keys(lib).sort(), declared);
  const esm = await import(pathToFileURL(require.resolve(pkg)).href);
  assert.deepEqual(Object.keys(esm).filter((key) => key !== "default" && key !== "module.exports").sort(), declared);
  assert.equal(esm.fromHtml, lib.fromHtml);
});
