"use strict";
// The wasm-bindgen glue (docrs.js) keeps its instance in a variable of its
// own with no way to replace it, and a panic leaves that instance poisoned.
// This file evaluates the glue as a function and calls it again for a
// fresh instance once the current one is poisoned, or has grown past
// HIGH_WATER. Font sets live in an instance's memory: each handle keeps its
// files and registers them again on the next instance that needs them.
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const glueFile = path.resolve(__dirname, "docrs.js");
const wasmFile = path.resolve(__dirname, "docrs_bg.wasm");
const compiled = new WebAssembly.Module(fs.readFileSync(wasmFile));
const glue = vm.compileFunction(
  fs.readFileSync(glueFile, "utf8"),
  ["exports", "require", "__dirname", "WebAssembly"],
  { filename: glueFile },
);

// WebAssembly memory never shrinks: after a call leaves the instance past
// this, the next call runs on a fresh one. Measured in Node 26: conversions
// with a 16 MiB font set stay under 50 MiB; the largest the serverless
// budgets allow (384 KiB of table HTML, 12 MiB of pictures) leave 142 MiB.
const HIGH_WATER = 128 << 20;

const GLUE = [
  "fontLimits",
  "freeFonts",
  "fromHtml",
  "fromJson",
  "fromMarkdown",
  "fromTiptap",
  "inspectReference",
  "markdownStyles",
  "referenceDocument",
  "registerFonts",
  "start",
  "version",
];

const unexpected = (what) =>
  new Error(`docrs-wasm: docrs.js ${what}; index.js expects the shape of wasm-bindgen's Node output`);

// An exception that unwinds through the module's frames, whatever it is,
// leaves its stack pointer behind: the instance is poisoned.
function marking(exports, state) {
  const marked = {};
  for (const [name, value] of Object.entries(exports)) {
    marked[name] =
      typeof value !== "function"
        ? value
        : function (...args) {
            try {
              return value(...args);
            } catch (error) {
              state.poisoned = true;
              throw error;
            }
          };
  }
  return marked;
}

let generations = 0;

function instantiate() {
  const fresh = { glue: null, wasm: null, poisoned: false, generation: ++generations, fontIds: new WeakMap() };
  // The glue's `fs` and `WebAssembly`: the file is not read again and the
  // module not compiled again.
  const files = {
    readFileSync(file) {
      if (path.resolve(String(file)) !== wasmFile) throw unexpected(`reads ${file}`);
      return null;
    },
  };
  const requires = (id) => (id === "fs" ? files : require(id));
  const namespace = Object.create(WebAssembly, {
    Module: { value: function Module() { return compiled; } },
    Instance: {
      value: function Instance(module, imports) {
        const instance = new WebAssembly.Instance(module, imports);
        fresh.wasm = marking(instance.exports, fresh);
        return Object.defineProperty(instance, "exports", { value: fresh.wasm });
      },
    },
  });
  const exports = {};
  glue(exports, requires, __dirname, namespace);
  const exported = Object.keys(exports).filter(
    (name) => typeof exports[name] === "function" && !name.startsWith("__"),
  );
  if (fresh.wasm === null || String(exported.sort()) !== String(GLUE)) {
    throw unexpected(`exports ${exported.join(", ") || "nothing"} where index.js guards ${GLUE.join(", ")}`);
  }
  fresh.glue = exports;
  return fresh;
}

let current = null;

const live = () => (current !== null && !current.poisoned ? current : (current = instantiate()));

// Run `body` on the live instance; replace the instance once a call
// poisons it or leaves it past HIGH_WATER.
function onInstance(body) {
  const used = live();
  try {
    const result = body(used);
    if (current === used && used.wasm.memory.buffer.byteLength > HIGH_WATER) current = null;
    return result;
  } catch (error) {
    if (used.poisoned) {
      // V8 holds the instance through the unformatted stack trace.
      void error.stack;
      if (current === used) {
        current = null;
        try {
          live();
        } catch {
          // The next call instantiates again and reports it.
        }
      }
    }
    throw error;
  }
}

function guarded(name) {
  const call = (...args) => onInstance((used) => used.glue[name](...args));
  Object.defineProperty(call, "name", { value: name });
  return call;
}

function docrsError(message, fields) {
  return Object.assign(new Error(message), { refusal: false, limit: null, ...fields });
}

const invalid = (reason, message) => docrsError(message, { code: "invalid_argument", reason });

const handles = new WeakMap();

// Where a handle was last registered, by instance generation: holding the
// instance itself would keep a replaced one alive.
const isLive = (ledger) => current !== null && !current.poisoned && ledger.generation === current.generation;

// Best effort: frees a collected handle's set if its instance is still live.
const collected = new FinalizationRegistry((ledger) => {
  if (isLive(ledger)) current.glue.freeFonts(ledger.id);
});

class FontSet {
  constructor(files, options, faces) {
    this.faces = Object.freeze(faces.map(Object.freeze));
    const state = { files, options, ledger: { generation: 0, id: 0 } };
    handles.set(this, state);
    collected.register(this, state.ledger, state);
    Object.freeze(this);
  }

  free() {
    const state = handles.get(this);
    if (state === undefined || state.files === null) return;
    state.files = null;
    collected.unregister(state);
    if (isLive(state.ledger)) {
      current.fontIds.delete(this);
      onInstance((used) => used.glue.freeFonts(state.ledger.id));
    }
  }
}

// The registry id of `fonts` in instance `used`, registering it there first
// if it is not yet.
function fontsIn(used, fonts) {
  const state = handles.get(fonts);
  if (state === undefined) {
    throw invalid("invalid_options", "fonts must be a font set from loadFonts or loadFontManifest");
  }
  if (state.files === null) {
    throw invalid("pdf_font_handle_disposed", "fonts: this font set was freed");
  }
  let id = used.fontIds.get(fonts);
  if (id === undefined) {
    id = used.glue.registerFonts(state.files, state.options).id;
    used.fontIds.set(fonts, id);
    Object.assign(state.ledger, { generation: used.generation, id });
  }
  return id;
}

function converting(name) {
  const call = (source, options, ...rest) =>
    onInstance((used) => {
      const fonts = options?.fonts;
      const given = fonts === undefined || fonts === null ? options : { ...options, fonts: fontsIn(used, fonts) };
      return used.glue[name](source, given, ...rest);
    });
  Object.defineProperty(call, "name", { value: name });
  return call;
}

// A handle for `files`, once the instance accepts them; `owned` files are
// the package's own and kept as they are, others are copied.
function fontSet(files, options, owned) {
  const kept = options === undefined || options === null ? undefined : { ...options };
  if (kept?.aliases) kept.aliases = { ...kept.aliases };
  return onInstance((used) => {
    const { id, faces } = used.glue.registerFonts(files, kept);
    const fonts = new FontSet(owned ? files : files.map((file) => new Uint8Array(file)), kept, faces);
    used.fontIds.set(fonts, id);
    Object.assign(handles.get(fonts).ledger, { generation: used.generation, id });
    return fonts;
  });
}

function loadFonts(files, options) {
  if (!Array.isArray(files) || !files.every((file) => file instanceof Uint8Array)) {
    throw invalid("invalid_argument", "files must be an array of Uint8Array");
  }
  return fontSet(files, options, false);
}

// Fetched font files by manifest id, least recently used first.
const fetched = new Map();
const FETCHED_BYTES = 64 << 20;
let fetchedBytes = 0;

function remember(id, bytes) {
  if (fetched.has(id)) return;
  fetched.set(id, bytes);
  fetchedBytes += bytes.byteLength;
  for (const [old, oldBytes] of fetched) {
    if (fetchedBytes <= FETCHED_BYTES) break;
    fetched.delete(old);
    fetchedBytes -= oldBytes.byteLength;
  }
}

function recall(id) {
  const bytes = fetched.get(id);
  if (bytes !== undefined) {
    fetched.delete(id);
    fetched.set(id, bytes);
  }
  return bytes;
}

const styleOf = (entry) =>
  entry.bold && entry.italic ? "bold italic" : entry.bold ? "bold" : entry.italic ? "italic" : "regular";

const named = (entry, error) =>
  Object.assign(error, {
    message: `font ${JSON.stringify(entry.id)} (${entry.family}, ${styleOf(entry)}): ${error.message}`,
    id: entry.id,
    family: entry.family,
    style: styleOf(entry),
  });

const tooLarge = (limit, allowed, observed) =>
  docrsError(`resource budget ${limit} exceeded: allowed ${allowed}, observed at least ${observed}`, {
    code: "resource_limit",
    reason: limit,
    limit: { limit, allowed, observed },
  });

const MANIFEST_OPTIONS = ["fetch", "signal", "timeoutMs", "maxBytes", "limits", "aliases"];

function checkEntry(entry, at) {
  const where = `entries[${at}]`;
  if (entry === null || typeof entry !== "object") throw invalid("invalid_options", `${where} must be an object`);
  if (typeof entry.id !== "string" || entry.id === "") throw invalid("invalid_options", `${where}.id must be a non-empty string`);
  if (typeof entry.family !== "string" || entry.family === "") {
    throw invalid("invalid_options", `${where}.family must be a non-empty string`);
  }
  if ((entry.url === undefined) === (entry.data === undefined)) {
    throw invalid("invalid_options", `${where} needs exactly one of url and data`);
  }
  if (entry.data !== undefined && !(entry.data instanceof Uint8Array)) {
    throw invalid("invalid_options", `${where}.data must be a Uint8Array`);
  }
  if (entry.url !== undefined) {
    let url;
    try {
      url = new URL(entry.url);
    } catch {
      throw named(entry, invalid("font_url_invalid", "url is not a URL"));
    }
    if (url.protocol !== "https:") throw named(entry, invalid("font_url_not_https", "fonts are fetched over https only"));
  }
}

async function readCapped(response, cap, spent) {
  const declared = Number(response.headers?.get?.("content-length"));
  if (Number.isFinite(declared) && declared > cap.file) throw tooLarge("max_font_file_bytes", cap.file, declared);
  const chunks = [];
  let size = 0;
  const take = (chunk) => {
    size += chunk.byteLength;
    spent.bytes += chunk.byteLength;
    if (size > cap.file) throw tooLarge("max_font_file_bytes", cap.file, size);
    if (spent.bytes > cap.set) throw tooLarge("max_font_set_bytes", cap.set, spent.bytes);
    chunks.push(chunk);
  };
  if (response.body?.getReader) {
    const reader = response.body.getReader();
    try {
      for (let read = await reader.read(); !read.done; read = await reader.read()) take(read.value);
    } finally {
      reader.cancel().catch(() => {});
    }
  } else {
    take(new Uint8Array(await response.arrayBuffer()));
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.byteLength;
  }
  return bytes;
}

// Settles with `promise`, or rejects with the signal's reason once it aborts,
// whether or not the fetch implementation honours the signal.
function abortable(promise, signal) {
  promise.catch(() => {});
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason);
    signal.addEventListener("abort", abort, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
  });
}

async function fetchFont(entry, fetching, signal, cap, spent) {
  let response;
  try {
    response = await abortable(Promise.resolve().then(() => fetching(entry.url, { signal })), signal);
  } catch (error) {
    if (signal.aborted) throw signal.reason;
    throw docrsError(`fetch failed: ${error?.message ?? error}`, { code: "io", reason: "font_fetch_failed", cause: error });
  }
  if (response.url && new URL(response.url).protocol !== "https:") {
    throw invalid("font_url_not_https", "the font was redirected away from https");
  }
  if (!response.ok) {
    throw docrsError(`fetch failed with HTTP ${response.status}`, { code: "io", reason: "font_fetch_failed" });
  }
  return abortable(readCapped(response, cap, spent), signal);
}

const timedOut = (timeoutMs) =>
  docrsError(`fetching fonts took longer than ${timeoutMs} ms`, { code: "io", reason: "font_fetch_timeout" });

// The files of `entries`, from `data`, the cache or https, within `cap`.
async function filesOf(entries, { fetching, signal, timeoutMs, cap }) {
  const controller = new AbortController();
  const timeout = timedOut(timeoutMs);
  const timer = setTimeout(() => controller.abort(timeout), timeoutMs);
  const forward = () => controller.abort(signal.reason);
  if (signal?.aborted) forward();
  signal?.addEventListener("abort", forward, { once: true });
  const spent = { bytes: 0 };
  const one = async (entry) => {
    const known = entry.data ?? recall(entry.id);
    if (known === undefined) return fetchFont(entry, fetching, controller.signal, cap, spent);
    spent.bytes += known.byteLength;
    if (known.byteLength > cap.file) throw tooLarge("max_font_file_bytes", cap.file, known.byteLength);
    if (spent.bytes > cap.set) throw tooLarge("max_font_set_bytes", cap.set, spent.bytes);
    return known;
  };
  try {
    return await Promise.all(
      entries.map((entry) =>
        one(entry).catch((error) => {
          if (controller.signal.aborted && error === controller.signal.reason) {
            // The caller's abort, or another entry's failure, as it is.
            if (error !== timeout) throw error;
            error = timedOut(timeoutMs);
          } else if (!(error instanceof Error)) {
            error = docrsError(String(error), { code: "io", reason: "font_fetch_failed" });
          }
          const failure = named(entry, error);
          controller.abort(failure);
          throw failure;
        }),
      ),
    );
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", forward);
  }
}

async function loadFontManifest(entries, options = {}) {
  if (!Array.isArray(entries)) throw invalid("invalid_options", "entries must be an array");
  for (const key of Object.keys(options ?? {})) {
    if (!MANIFEST_OPTIONS.includes(key)) throw invalid("invalid_options", `unknown option ${JSON.stringify(key)}`);
  }
  const { fetch: fetching = globalThis.fetch, signal, timeoutMs = 30_000, maxBytes, limits, aliases } = options ?? {};
  entries.forEach(checkEntry);
  if (!(Number.isFinite(timeoutMs) && timeoutMs > 0)) throw invalid("invalid_options", "timeoutMs must be a positive number");
  if (maxBytes !== undefined && !(Number.isSafeInteger(maxBytes) && maxBytes >= 0)) {
    throw invalid("invalid_options", "maxBytes must be a whole number of bytes");
  }
  const budget = onInstance((used) => used.glue.fontLimits(limits));
  const cap = { file: Math.min(budget.fileBytes, maxBytes ?? Infinity), set: Math.min(budget.setBytes, maxBytes ?? Infinity) };
  const wanted = [];
  const ids = new Set();
  for (const entry of entries) {
    if (!ids.has(entry.id)) wanted.push(entry);
    ids.add(entry.id);
  }
  const files = await filesOf(wanted, { fetching, signal, timeoutMs, cap });
  let fonts;
  try {
    fonts = fontSet(files, { limits, aliases }, true);
  } catch (error) {
    throw Number.isInteger(error.file) ? named(wanted[error.file], error) : error;
  }
  wanted.forEach((entry, at) => {
    if (entry.url !== undefined) remember(entry.id, files[at]);
  });
  return fonts;
}

// One assignment per name: Node reads the named exports of a CommonJS
// module from its source.
exports.fromHtml = converting("fromHtml");
exports.fromJson = converting("fromJson");
exports.fromMarkdown = converting("fromMarkdown");
exports.fromTiptap = converting("fromTiptap");
exports.inspectReference = guarded("inspectReference");
exports.loadFontManifest = loadFontManifest;
exports.loadFonts = loadFonts;
exports.markdownStyles = guarded("markdownStyles");
exports.referenceDocument = guarded("referenceDocument");
exports.start = guarded("start");
exports.version = guarded("version");

// For tests and diagnostics; not part of the documented interface.
Object.defineProperty(exports, "__wasm", { get: () => live().wasm });

current = instantiate();
