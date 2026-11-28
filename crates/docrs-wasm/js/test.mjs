// Node tests of the built package (crates/docrs-wasm/pkg, or the installed
// tarball in DOCRS_WASM_PKG; DOCRS_TSC names a tsc for the typings test):
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

test("fields fill {{Name}} merge fields", () => {
  const { docx, report } = lib.fromHtml("<p>Dear <strong>{{Cli</strong>ent}}, {{ Ref }}{{Other}}</p>", {
    fields: { Client: "Jo", Ref: "" },
  });
  assert.deepEqual(report.fields, { filled: 2, left: ["Other"], refused: [] });
  assert.equal(lib.fromHtml("<p>x</p>").report.fields, null);
  assert.ok(isZip(docx));
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

test("contents page numbers and report.toc follow the fields filled in", () => {
  const filler = "Filler text that takes a full line of the page body here.\n\n".repeat(56);
  const source = `# Intro\n\nDear {{Client}},\n\n${filler}# Late\n\nEnd.\n`;
  const latePage = (fields) => {
    const { docx, report } = lib.fromMarkdown(source, { toc: true, fields });
    const xml = documentXml(docx);
    const rest = xml.slice(xml.indexOf(" PAGEREF late "));
    const number = rest.slice(rest.indexOf("<w:t>") + 5);
    return [number.slice(0, number.indexOf("<")), report.toc.pages, report.toc.entries];
  };
  assert.deepEqual(latePage({}), ["3", 3, 2]);
  assert.deepEqual(latePage({ Client: "Jo Smith of a long merge value ".repeat(12) }), ["4", 4, 2]);
});

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

test("the WebAssembly module stays within its 3.9 MB budget", () => {
  const size = readFileSync(join(pkg, "docrs_bg.wasm")).length;
  assert.ok(size <= 3_900_000, `docrs_bg.wasm is ${size} bytes`);
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

// Font sets (experimental).
const fontFile = (path) => readBytes(join(root, "tests", "fixtures", "fonts", path));
const INTER = fontFile("inter/Inter[opsz,wght].ttf");
const SERIF = fontFile("liberation/LiberationSerif-Regular.ttf");
const INTER_REFERENCE = readBytes(join(root, "tests", "fixtures", "pdf", "references", "inter.docx"));
// Inter sets wider than Calibri, which measures a missing font: with Inter
// supplied the later headings move to later pages.
const WIDE =
  Array.from({ length: 8 }, (_, i) => `# Section ${i + 1}\n\n`).join(
    "Inter sets wider than the substitute Word uses when the font is missing, so each of these paragraphs wraps onto more lines and the later headings move to later pages.\n\n".repeat(6),
  ) + "End.\n";
const contents = (fonts, options = {}) => {
  const { docx, report } = lib.fromMarkdown(WIDE, { reference: INTER_REFERENCE, toc: true, fonts, ...options });
  const numbers = documentXml(docx)
    .split(" PAGEREF ")
    .slice(1)
    .map((rest) => rest.slice(rest.indexOf("<w:t>") + 5, rest.indexOf("</w:t>", rest.indexOf("<w:t>"))));
  return { numbers, missing: report.toc.missing_fonts };
};
const disposed = (err) => err.code === "invalid_argument" && err.reason === "pdf_font_handle_disposed";
const limited = (limit) => (err) => err.code === "resource_limit" && err.limit.limit === limit;

test("a supplied font measures the contents numbers", () => {
  const fonts = lib.loadFonts([INTER, SERIF]);
  assert.deepEqual(
    fonts.faces.map((face) => [face.family, face.variable]),
    [["Inter", true], ["Liberation Serif", false]],
  );
  assert.ok(Object.isFrozen(fonts) && Object.isFrozen(fonts.faces));
  const without = contents(undefined);
  const withInter = contents(fonts);
  assert.deepEqual(without.missing, ["Inter"]);
  assert.deepEqual(withInter.missing, []);
  assert.equal(without.numbers.length, 8);
  assert.notDeepEqual(withInter.numbers, without.numbers);
  const html = lib.fromHtml("<h1>A</h1><p>x</p>", { reference: INTER_REFERENCE, toc: true, fonts }).report.toc;
  const json = lib.fromJson('{"type":"doc","content":[{"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"A"}]}]}', {
    reference: INTER_REFERENCE,
    toc: true,
    fonts,
  }).report.toc;
  const tiptap = lib.fromTiptap('{"type":"doc","content":[{"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"A"}]}]}', {
    reference: INTER_REFERENCE,
    toc: true,
    fonts,
  }).report.toc;
  for (const toc of [html, json, tiptap]) assert.deepEqual(toc.missing_fonts, []);
  assert.deepEqual(lib.fromMarkdown("# A\n", { toc: true }).report.toc.missing_fonts, []);
  fonts.free();
});

test("one bad file refuses the whole set and is named", () => {
  assert.throws(
    () => lib.loadFonts([INTER, new Uint8Array([1, 2, 3, 4])]),
    (err) => err.code === "invalid_argument" && err.reason === "font_format_unsupported" && err.file === 1 && /^fonts\[1\]: /.test(err.message),
  );
  assert.throws(() => lib.loadFonts(INTER), (err) => err.code === "invalid_argument");
  assert.throws(() => lib.loadFonts([INTER], { limits: "huge" }), (err) => err.code === "invalid_argument");
  assert.throws(() => lib.loadFonts([INTER], { nonsense: 1 }), (err) => err.reason === "invalid_options");
  assert.throws(
    () => lib.fromMarkdown("x", { fonts: { faces: [], free() {} } }),
    (err) => err.code === "invalid_argument" && err.reason === "invalid_options",
  );
  const fonts = lib.loadFonts([SERIF]);
  assert.throws(() => lib.inspectReference(INTER_REFERENCE, { fonts }), (err) => /inspectReference takes no fonts/.test(err.message));
  assert.throws(() => lib.markdownStyles(undefined, undefined, { fonts }), (err) => /markdownStyles takes no fonts/.test(err.message));
  fonts.free();
});

test("a font set survives the instance being replaced", () => {
  const fonts = lib.loadFonts([INTER]);
  const before = contents(fonts);
  const instance = lib.__wasm;
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.notEqual(lib.__wasm, instance);
  assert.deepEqual(contents(fonts), before);
  assert.deepEqual(before.missing, []);
  fonts.free();
});

test("a font set keeps its own copy of the files", () => {
  const file = Buffer.from(SERIF);
  const fonts = lib.loadFonts([file]);
  file.fill(0);
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts }).docx), "registered again from its own copy");
  fonts.free();
});

test("a live font set keeps no replaced instance reachable", async () => {
  const fonts = lib.loadFonts([SERIF]);
  const replaced = new WeakRef(lib.__wasm.memory);
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts }).docx));
  await timers.setImmediate();
  collect();
  await timers.setImmediate();
  collect();
  assert.equal(replaced.deref(), undefined);
  fonts.free();
});

test("two tenants' font sets never mix", () => {
  const inter = lib.loadFonts([INTER]);
  const serif = lib.loadFonts([SERIF]);
  for (let round = 0; round < 3; round++) {
    assert.deepEqual(contents(serif).missing, ["Inter"]);
    assert.deepEqual(contents(inter).missing, []);
    assert.deepEqual(contents(serif).missing, ["Inter"]);
    if (round === 1) assert.throws(trapInside, WebAssembly.RuntimeError);
  }
  inter.free();
  assert.throws(() => contents(inter), disposed);
  assert.deepEqual(contents(serif).missing, ["Inter"]);
  serif.free();
});

test("a freed font set throws pdf_font_handle_disposed, after a reset too", () => {
  const fonts = lib.loadFonts([SERIF]);
  fonts.free();
  fonts.free();
  assert.throws(() => contents(fonts), disposed);
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.throws(() => lib.fromHtml("<p>x</p>", { fonts }), disposed);
  assert.equal(fonts.faces[0].family, "Liberation Serif");
});

test("a set loaded under default budgets is checked against each call's", () => {
  assert.throws(trapInside, WebAssembly.RuntimeError);
  const many = lib.loadFonts(Array(25).fill(SERIF));
  assert.equal(many.faces.length, 25);
  assert.ok(isZip(lib.fromMarkdown("x", { fonts: many }).docx));
  assert.throws(() => lib.fromMarkdown("x", { fonts: many, limits: "serverless" }), limited("max_font_faces"));
  assert.throws(() => lib.loadFonts(Array(25).fill(SERIF), { limits: "serverless" }), limited("max_font_faces"));
  assert.throws(() => lib.loadFonts([new Uint8Array((8 << 20) + 1)], { limits: "serverless" }), limited("max_font_file_bytes"));
  assert.throws(
    () => lib.loadFonts([new Uint8Array(6 << 20), new Uint8Array(6 << 20), new Uint8Array(6 << 20)], { limits: "serverless" }),
    limited("max_font_set_bytes"),
  );
  many.free();
  const sets = Array.from({ length: 8 }, () => lib.loadFonts([SERIF], { limits: "serverless" }));
  assert.throws(() => lib.loadFonts([SERIF], { limits: "serverless" }), limited("max_font_sets"));
  // Handles count across instances: a reset frees no room.
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.throws(() => lib.loadFonts([SERIF], { limits: "serverless" }), limited("max_font_sets"));
  for (const set of sets) assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts: set, limits: "serverless" }).docx));
  sets.pop().free();
  sets.push(lib.loadFonts([SERIF], { limits: "serverless" }));
  assert.throws(trapInside, WebAssembly.RuntimeError);
  for (const set of sets) assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts: set, limits: "serverless" }).docx));
  // Under the default budgets more handles live, and every one still converts after a reset.
  const more = Array.from({ length: 12 }, () => lib.loadFonts([SERIF]));
  assert.throws(trapInside, WebAssembly.RuntimeError);
  for (const set of [...sets, ...more]) assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts: set }).docx));
  for (const set of [...sets, ...more]) set.free();
  assert.throws(() => lib.loadFonts([SERIF], { limits: 1 }), (err) => err.reason === "invalid_options");
});

// A black PNG `side` pixels square, stored uncompressed.
const blackPng = (side) => {
  const { deflateSync, crc32 } = require("node:zlib");
  const chunk = (type, data) => {
    const out = Buffer.alloc(12 + data.length);
    out.writeUInt32BE(data.length, 0);
    out.write(type, 4, "latin1");
    data.copy(out, 8);
    out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(side, 0);
  header.writeUInt32BE(side, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const rows = Buffer.alloc(side * (side * 3 + 1));
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows, { level: 0 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  return new Uint8Array(png);
};

test("a large call is followed by a fresh instance, and alternating tenants keep memory flat", () => {
  const inter = lib.loadFonts([INTER]);
  const serif = lib.loadFonts([SERIF]);
  const picture = blackPng(1500);
  const images = Object.fromEntries(Array.from({ length: 24 }, (_, i) => [`p${i}.png`, picture]));
  const large = Object.keys(images).map((name) => `![](${name})\n\n`).join("");
  const instance = lib.__wasm;
  assert.equal(lib.fromMarkdown(large, { images, fonts: inter }).report.counts.images, 24);
  assert.notEqual(lib.__wasm, instance, "the instance past the high-water mark was replaced");
  assert.ok(lib.__wasm.memory.buffer.byteLength < 32 << 20);
  // Every hundredth call measures its contents with the tenant's font.
  const failing = { images, strict: true };
  const before = lib.__wasm;
  assert.throws(() => lib.fromMarkdown(`${large}![x](missing.png)\n`, failing), (err) => err.refusal === true);
  assert.notEqual(lib.__wasm, before, "a refusal past the high-water mark replaces the instance too");
  const small = (i) => {
    const fonts = i % 2 ? inter : serif;
    if (i % 100 !== 99) return lib.fromHtml(`<p>Call ${i}</p>`, { fonts });
    const { report } = lib.fromMarkdown(WIDE, { reference: INTER_REFERENCE, toc: true, fonts });
    assert.deepEqual(report.toc.missing_fonts, [], `call ${i}`);
  };
  for (let i = 0; i < 100; i++) small(i);
  const settled = { wasm: lib.__wasm.memory.buffer.byteLength, rss: process.memoryUsage().rss };
  for (let i = 100; i < 1000; i++) small(i);
  assert.equal(lib.__wasm.memory.buffer.byteLength, settled.wasm);
  assert.ok(process.memoryUsage().rss - settled.rss < 64 << 20, "resident memory grew over 1,000 calls");
  inter.free();
  serif.free();
});

// A fetch that serves `files` (URL → bytes) and counts what it was asked for.
const server = (files, { delay = 0, headers = {} } = {}) => {
  const asked = [];
  const fetch = async (url, { signal }) => {
    asked.push(url);
    if (delay) await timers.setTimeout(delay, undefined, { signal });
    if (!(url in files)) return new Response("missing", { status: 404 });
    return new Response(files[url], { headers });
  };
  return { fetch, asked };
};
const entry = (id, url, extra = {}) => ({ id, family: "Inter", url, ...extra });

test("a font manifest fetches over https only and names the entry in errors", async () => {
  const { fetch, asked } = server({ "https://fonts.example/inter.ttf": INTER, "https://fonts.example/serif.ttf": SERIF });
  const fonts = await lib.loadFontManifest(
    [entry("m1-inter", "https://fonts.example/inter.ttf"), { id: "m1-serif", family: "Liberation Serif", data: SERIF }],
    { fetch },
  );
  assert.deepEqual(asked, ["https://fonts.example/inter.ttf"]);
  assert.deepEqual(contents(fonts).missing, []);
  fonts.free();
  for (const url of ["http://fonts.example/inter.ttf", "file:///etc/passwd", "data:font/ttf;base64,AA=="]) {
    await assert.rejects(
      lib.loadFontManifest([entry("m1-plain", url, { bold: true })], { fetch }),
      (err) => err.reason === "font_url_not_https" && err.id === "m1-plain" && err.family === "Inter" && err.style === "bold",
    );
  }
  await assert.rejects(lib.loadFontManifest([entry("m1-bad", "not a url")], { fetch }), (err) => err.reason === "font_url_invalid");
  let reads = 0;
  const shifty = { toString: () => (reads++ ? "http://internal.example/a.ttf" : "https://fonts.example/inter.ttf") };
  await assert.rejects(lib.loadFontManifest([entry("m1-object", shifty)], { fetch }), (err) => err.reason === "font_url_invalid" && err.id === "m1-object");
  assert.equal(reads, 0);
  assert.equal(asked.length, 1);
  const inits = [];
  const redirecting = (to, flag) => async (url, init) => {
    inits.push(init);
    const response = new Response(INTER);
    Object.defineProperty(response, "url", { value: to });
    if (flag) Object.defineProperty(response, "redirected", { value: true });
    return response;
  };
  for (const fetch of [
    redirecting("http://fonts.example/inter.ttf"),
    redirecting("https://169.254.169.254/latest"),
    redirecting("", true),
  ]) {
    await assert.rejects(
      lib.loadFontManifest([entry("m1-redirect", "https://fonts.example/inter.ttf")], { fetch }),
      (err) => err.reason === "font_url_redirected" && err.id === "m1-redirect",
    );
  }
  assert.ok(inits.every((init) => init.redirect === "error"));
  // A fragment is not sent, so a response without it is no redirect.
  const asks = [];
  const plain = async (url) => {
    asks.push(url);
    return Object.defineProperty(new Response(INTER), "url", { value: "https://fonts.example/inter.ttf" });
  };
  const versioned = await lib.loadFontManifest([entry("m1-fragment", "https://fonts.example/inter.ttf#v2")], { fetch: plain });
  assert.deepEqual(asks, ["https://fonts.example/inter.ttf"]);
  versioned.free();
  await assert.rejects(
    lib.loadFontManifest([entry("m1-404", "https://fonts.example/none.ttf", { italic: true })], { fetch }),
    (err) => err.code === "io" && err.reason === "font_fetch_failed" && /"m1-404" \(Inter, italic\)/.test(err.message),
  );
  const broken = server({ "https://fonts.example/broken.ttf": new Uint8Array([1, 2, 3, 4]) });
  await assert.rejects(
    lib.loadFontManifest([entry("m1-ok", "https://fonts.example/inter.ttf"), entry("m1-broken", "https://fonts.example/broken.ttf", { bold: true, italic: true })], {
      fetch: async (url, init) => (url.includes("broken") ? broken.fetch(url, init) : fetch(url, init)),
    }),
    (err) => err.reason === "font_format_unsupported" && err.id === "m1-broken" && err.style === "bold italic",
  );
  await assert.rejects(lib.loadFontManifest([{ id: "x", family: "X" }]), (err) => err.reason === "invalid_options");
  await assert.rejects(lib.loadFontManifest([], { nonsense: 1 }), (err) => err.reason === "invalid_options");
});

test("a font manifest caps each file and the set in bytes", async () => {
  const { fetch } = server({ "https://fonts.example/inter.ttf": INTER, "https://fonts.example/serif.ttf": SERIF });
  await assert.rejects(
    lib.loadFontManifest([entry("m2-inter", "https://fonts.example/inter.ttf")], { fetch, maxBytes: 100_000 }),
    (err) => limited("max_font_file_bytes")(err) && err.id === "m2-inter" && err.limit.allowed === 100_000,
  );
  const declared = server({ "https://fonts.example/inter.ttf": INTER }, { headers: { "content-length": String(64 << 20) } });
  await assert.rejects(
    lib.loadFontManifest([entry("m2-declared", "https://fonts.example/inter.ttf")], { fetch: declared.fetch, limits: "serverless" }),
    (err) => limited("max_font_file_bytes")(err) && err.limit.observed === 64 << 20,
  );
  // A response without a stream (node-fetch 2) is capped by its length.
  const buffered = (headers) => async () => ({ ok: true, status: 200, headers: new Headers(headers), arrayBuffer: async () => INTER.slice().buffer });
  await assert.rejects(
    lib.loadFontManifest([entry("m2-buffered", "https://fonts.example/inter.ttf")], { fetch: buffered({}), maxBytes: 1000 }),
    (err) => limited("max_font_file_bytes")(err) && err.limit.observed === INTER.length,
  );
  await assert.rejects(
    lib.loadFontManifest([entry("m2-declared-buffered", "https://fonts.example/inter.ttf")], {
      fetch: buffered({ "content-length": String(INTER.length) }),
      maxBytes: 1000,
    }),
    (err) => limited("max_font_file_bytes")(err),
  );
  const whole = await lib.loadFontManifest([entry("m2-whole", "https://fonts.example/inter.ttf")], { fetch: buffered({}) });
  whole.free();
  await assert.rejects(
    lib.loadFontManifest([entry("m2-a", "https://fonts.example/inter.ttf"), entry("m2-b", "https://fonts.example/serif.ttf")], {
      fetch,
      maxBytes: INTER.length + 1000,
    }),
    limited("max_font_set_bytes"),
  );
});

test("a font manifest times out and honours an AbortSignal", async () => {
  const slow = server({ "https://fonts.example/inter.ttf": INTER }, { delay: 5_000 });
  const started = Date.now();
  await assert.rejects(
    lib.loadFontManifest([entry("m3-slow", "https://fonts.example/inter.ttf")], { fetch: slow.fetch, timeoutMs: 50 }),
    (err) => err.code === "io" && err.reason === "font_fetch_timeout" && err.id === "m3-slow",
  );
  assert.ok(Date.now() - started < 2_000);
  for (const timeoutMs of [2 ** 31, 1e10, Infinity, 0, -1, NaN]) {
    await assert.rejects(
      lib.loadFontManifest([entry("m3-long", "https://fonts.example/inter.ttf")], { fetch: slow.fetch, timeoutMs }),
      (err) => err.reason === "invalid_options",
      String(timeoutMs),
    );
  }
  const deaf = () => new Promise(() => {});
  await assert.rejects(
    lib.loadFontManifest([entry("m3-deaf", "https://fonts.example/inter.ttf")], { fetch: deaf, timeoutMs: 50 }),
    (err) => err.reason === "font_fetch_timeout",
  );
  const controller = new AbortController();
  const reason = new Error("tenant left");
  setTimeout(() => controller.abort(reason), 20);
  await assert.rejects(
    lib.loadFontManifest([entry("m3-abort", "https://fonts.example/inter.ttf")], { fetch: slow.fetch, signal: controller.signal }),
    (err) => err === reason,
  );
  await assert.rejects(
    lib.loadFontManifest([entry("m3-aborted", "https://fonts.example/inter.ttf")], { fetch: slow.fetch, signal: AbortSignal.abort(reason) }),
    (err) => err === reason,
  );
});

test("a font manifest caches files by id in the caller's cache, never by URL", async () => {
  const { fetch, asked } = server({ "https://a.example/inter.ttf": INTER, "https://b.example/inter.ttf": INTER });
  const cache = lib.createFontCache();
  const one = await lib.loadFontManifest(
    [entry("m4-inter", "https://a.example/inter.ttf"), entry("m4-inter", "https://b.example/inter.ttf")],
    { fetch, cache },
  );
  assert.deepEqual(asked, ["https://a.example/inter.ttf"]);
  assert.equal(one.faces.length, 1);
  const again = await lib.loadFontManifest([entry("m4-inter", "https://b.example/inter.ttf")], { fetch, cache });
  assert.equal(asked.length, 1);
  const two = await lib.loadFontManifest(
    [entry("m4-x", "https://a.example/inter.ttf"), entry("m4-y", "https://a.example/inter.ttf")],
    { fetch, cache },
  );
  assert.deepEqual(asked.slice(1), ["https://a.example/inter.ttf", "https://a.example/inter.ttf"]);
  assert.equal(two.faces.length, 2);
  // Another tenant's cache, or none, shares nothing.
  const other = await lib.loadFontManifest([entry("m4-inter", "https://b.example/inter.ttf")], { fetch, cache: lib.createFontCache() });
  const uncached = await lib.loadFontManifest([entry("m4-inter", "https://b.example/inter.ttf")], { fetch });
  assert.deepEqual(asked.slice(3), ["https://b.example/inter.ttf", "https://b.example/inter.ttf"]);
  const small = lib.createFontCache({ maxBytes: 1000 });
  await lib.loadFontManifest([entry("m4-small", "https://a.example/inter.ttf")], { fetch, cache: small });
  await lib.loadFontManifest([entry("m4-small", "https://a.example/inter.ttf")], { fetch, cache: small });
  assert.equal(asked.length, 7, "a file over the cache's size is not kept");
  await assert.rejects(lib.loadFontManifest([], { cache: new Map() }), (err) => err.reason === "invalid_options");
  assert.throws(() => lib.createFontCache({ maxBytes: -1 }), (err) => err.reason === "invalid_options");
  for (const set of [one, again, two, other, uncached]) set.free();
});

test("a manifest's data is copied, and its file must hold the declared family and style", async () => {
  const data = Buffer.from(SERIF);
  const fonts = await lib.loadFontManifest([{ id: "m5-serif", family: "Liberation Serif", data }]);
  data.fill(0);
  assert.throws(trapInside, WebAssembly.RuntimeError);
  assert.ok(isZip(lib.fromHtml("<p>x</p>", { fonts }).docx), "registered again from its own copy");
  fonts.free();
  for (const [declared, style] of [[{ family: "Arial" }, "regular"], [{ family: "Liberation Serif", bold: true, italic: true }, "bold italic"]]) {
    await assert.rejects(
      lib.loadFontManifest([{ id: "m5-wrong", ...declared, data: SERIF }]),
      (err) => err.reason === "font_face_mismatch" && err.id === "m5-wrong" && err.style === style && /Liberation Serif/.test(err.message),
    );
  }
  // A variable face covers both weights, not italics.
  const bold = await lib.loadFontManifest([{ id: "m5-inter-bold", family: "inter", bold: true, data: INTER }]);
  bold.free();
  await assert.rejects(
    lib.loadFontManifest([{ id: "m5-inter-italic", family: "Inter", italic: true, data: INTER }]),
    (err) => err.reason === "font_face_mismatch",
  );
});

// A consumer project around the installed package: `lib` its TypeScript libs.
async function compile(name, lib, source) {
  const { mkdtempSync, writeFileSync, mkdirSync, symlinkSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { execFileSync } = await import("node:child_process");
  const dir = mkdtempSync(join(tmpdir(), `docrs-${name}-`));
  mkdirSync(join(dir, "node_modules"));
  symlinkSync(pkg, join(dir, "node_modules", "docrs-wasm"), "dir");
  writeFileSync(
    join(dir, "tsconfig.json"),
    JSON.stringify({ compilerOptions: { strict: true, noEmit: true, module: "nodenext", moduleResolution: "nodenext", target: "es2022", lib, types: [] }, files: ["consumer.ts"] }),
  );
  writeFileSync(join(dir, "consumer.ts"), source);
  try {
    execFileSync(process.env.DOCRS_TSC, ["-p", dir], { stdio: "pipe" });
  } catch (err) {
    assert.fail(`${name}: ${err.stdout}`);
  }
}

const tsc = { skip: process.env.DOCRS_TSC ? false : "set DOCRS_TSC to a tsc binary" };

test("the typings compile against a consumer", tsc, () =>
  compile(
    "consumer",
    ["es2022", "dom"],
    `import { createFontCache, fromHtml, fromJson, fromMarkdown, fromTiptap, inspectReference, loadFontManifest, loadFonts, markdownStyles, type FontSet, type FromMarkdownResult } from "docrs-wasm";
const fonts: FontSet = loadFonts([new Uint8Array(4)], { limits: "serverless", aliases: { Brand: "Inter" } });
const family: string = fonts.faces[0].family;
const plain: FromMarkdownResult = fromMarkdown("# A", { fonts, toc: true });
fromHtml("<p>x</p>", { fonts });
fromJson({ type: "doc" }, { fonts });
fromTiptap("{}", { fonts });
inspectReference(new Uint8Array(4), { limits: "serverless" });
// @ts-expect-error a conversion without pdf has no pdf
plain.pdf;
// @ts-expect-error only a handle from loadFonts or loadFontManifest
fromMarkdown("x", { fonts: { faces: [], free() {} } });
// @ts-expect-error inspectReference takes no fonts
inspectReference(new Uint8Array(4), { fonts });
// @ts-expect-error markdownStyles takes no fonts
markdownStyles(undefined, undefined, { fonts });
// @ts-expect-error an entry has a url or data
loadFontManifest([{ id: "a", family: "Inter" }]);
// @ts-expect-error a cache comes from createFontCache
loadFontManifest([], { cache: new Map() });
const cache = createFontCache({ maxBytes: 1 << 20 });
async function manifest(signal: AbortSignal): Promise<FontSet> {
  return loadFontManifest([{ id: "a", family: "Inter", bold: true, url: "https://x.example/a.ttf" }, { id: "b", family: "Inter", data: new Uint8Array(4) }], { signal, cache, timeoutMs: 1000, maxBytes: 1 << 20, fetch: (url, init) => fetch(url, init) });
}
loadFontManifest([], { fetch, signal: AbortSignal.timeout(10) });
fonts.free();
void family; void manifest;
`,
  ),
);

test("the typings need no DOM lib", tsc, () =>
  compile(
    "plain",
    ["es2022"],
    `import { fromMarkdown, loadFontManifest, loadFonts } from "docrs-wasm";
const fonts = loadFonts([new Uint8Array(4)]);
fromMarkdown("# A", { fonts });
void loadFontManifest([{ id: "b", family: "Inter", data: new Uint8Array(4) }], { timeoutMs: 1000 });
`,
  ),
);
