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
// poisons it or leaves it past HIGH_WATER, whether the call succeeded or not.
function onInstance(body) {
  const used = live();
  try {
    return body(used);
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
  } finally {
    if (current === used && !used.poisoned && used.wasm.memory.buffer.byteLength > HIGH_WATER) current = null;
  }
}

function guarded(name) {
  const call = (...args) => onInstance((used) => used.glue[name](...args));
  Object.defineProperty(call, "name", { value: name });
  return call;
}

// `name` with its options (argument `at`) refusing `fonts`, which only
// conversions take.
function fontless(name, at) {
  const inner = guarded(name);
  const call = (...args) => {
    const fonts = args[at]?.fonts;
    if (fonts !== undefined && fonts !== null) throw invalid("invalid_options", `${name} takes no fonts`);
    return inner(...args);
  };
  Object.defineProperty(call, "name", { value: name });
  return call;
}

function docrsError(message, fields) {
  return Object.assign(new Error(message), { refusal: false, limit: null, ...fields });
}

const invalid = (reason, message) => docrsError(message, { code: "invalid_argument", reason });

const tooLarge = (limit, allowed, observed) =>
  docrsError(`resource budget ${limit} exceeded: allowed ${allowed}, observed at least ${observed}`, {
    code: "resource_limit",
    reason: limit,
    limit: { limit, allowed, observed },
  });

// The font budgets of a `limits` profile, as the module defines them.
const profiles = new Map();
function fontLimits(name) {
  if (name !== undefined && name !== null && typeof name !== "string") {
    throw invalid("invalid_options", 'limits must be "default" or "serverless"');
  }
  const key = name ?? "default";
  if (!profiles.has(key)) profiles.set(key, onInstance((used) => used.glue.fontLimits(key)));
  return profiles.get(key);
}

// Refuse a set by its sizes alone: `sizes.faces` is the number of files
// before they are read.
function checkSet(sizes, limits) {
  if (sizes.faces > limits.faces) throw tooLarge("max_font_faces", limits.faces, sizes.faces);
  if (sizes.largest > limits.fileBytes) throw tooLarge("max_font_file_bytes", limits.fileBytes, sizes.largest);
  if (sizes.bytes > limits.setBytes) throw tooLarge("max_font_set_bytes", limits.setBytes, sizes.bytes);
}

const sizesOf = (files) => ({
  faces: files.length,
  largest: Math.max(0, ...files.map((file) => file.byteLength)),
  bytes: files.reduce((sum, file) => sum + file.byteLength, 0),
});

// Font set handles not yet freed or collected, whatever instance they were
// last registered in: their copies of the files are what the caps bound.
const alive = { sets: 0, bytes: 0 };

function release(tally) {
  if (tally.released) return;
  tally.released = true;
  alive.sets -= 1;
  alive.bytes -= tally.bytes;
}

const handles = new WeakMap();

// Where a handle was last registered, by instance generation: holding the
// instance itself would keep a replaced one alive.
const isLive = (ledger) => current !== null && !current.poisoned && ledger.generation === current.generation;

// Best effort: a collected handle no longer counts, and its set is freed if
// its instance is still live.
const collected = new FinalizationRegistry(({ ledger, tally }) => {
  release(tally);
  if (isLive(ledger)) current.glue.freeFonts(ledger.id);
});

class FontSet {
  constructor(files, options, faces, sizes, used, id) {
    this.faces = Object.freeze(faces.map(Object.freeze));
    const tally = { bytes: sizes.bytes, released: false };
    const state = { files, options, sizes: { ...sizes, faces: faces.length }, ledger: { generation: used.generation, id }, tally };
    alive.sets += 1;
    alive.bytes += sizes.bytes;
    handles.set(this, state);
    used.fontIds.set(this, id);
    collected.register(this, { ledger: state.ledger, tally }, state);
    Object.freeze(this);
  }

  free() {
    const state = handles.get(this);
    if (state === undefined || state.files === null) return;
    state.files = null;
    collected.unregister(state);
    release(state.tally);
    if (isLive(state.ledger)) {
      current.fontIds.delete(this);
      onInstance((used) => used.glue.freeFonts(state.ledger.id));
    }
  }
}

// The registry id of `fonts` in instance `used`, registering it there first
// if it is not yet, once the set fits the call's budgets.
function fontsIn(used, fonts, limits) {
  const state = handles.get(fonts);
  if (state === undefined) {
    throw invalid("invalid_options", "fonts must be a font set from loadFonts or loadFontManifest");
  }
  if (state.files === null) {
    throw invalid("pdf_font_handle_disposed", "fonts: this font set was freed");
  }
  if (limits !== null) checkSet(state.sizes, limits);
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
      if (fonts === undefined || fonts === null) return used.glue[name](source, options, ...rest);
      // A `limits` that is not a profile name is refused by the call itself.
      const profile = options.limits ?? undefined;
      const limits = profile === undefined || typeof profile === "string" ? fontLimits(profile) : null;
      return used.glue[name](source, { ...options, fonts: fontsIn(used, fonts, limits) }, ...rest);
    });
  Object.defineProperty(call, "name", { value: name });
  return call;
}

// A handle for `files` once the instance accepts them and `accept(faces,
// perFile)` returns. Files are checked by size and copied first, unless
// `owned` (the package's own).
function fontSet(files, options, { owned = false, accept } = {}) {
  const kept = options === undefined || options === null ? undefined : { ...options };
  if (kept?.aliases) kept.aliases = { ...kept.aliases };
  const limits = fontLimits(kept?.limits);
  const sizes = sizesOf(files);
  checkSet(sizes, limits);
  if (alive.sets >= limits.sets) throw tooLarge("max_font_sets", limits.sets, alive.sets + 1);
  const room = limits.sets * limits.setBytes;
  if (alive.bytes + sizes.bytes > room) throw tooLarge("max_font_live_bytes", room, alive.bytes + sizes.bytes);
  const copies = owned ? files : files.map((file) => new Uint8Array(file));
  return onInstance((used) => {
    const { id, faces, perFile } = used.glue.registerFonts(copies, kept);
    try {
      accept?.(faces, perFile);
    } catch (error) {
      used.glue.freeFonts(id);
      throw error;
    }
    return new FontSet(copies, kept, faces, sizes, used, id);
  });
}

function loadFonts(files, options) {
  if (!Array.isArray(files) || !files.every((file) => file instanceof Uint8Array)) {
    throw invalid("invalid_argument", "files must be an array of Uint8Array");
  }
  return fontSet(files, options);
}

const caches = new WeakMap();

class FontCache {
  constructor(maxBytes) {
    caches.set(this, { files: new Map(), bytes: 0, maxBytes });
    Object.freeze(this);
  }
}

function createFontCache(options) {
  const maxBytes = options?.maxBytes ?? 64 << 20;
  if (!(Number.isSafeInteger(maxBytes) && maxBytes >= 0)) throw invalid("invalid_options", "maxBytes must be a whole number of bytes");
  return new FontCache(maxBytes);
}

// Least recently used first.
function remember(cache, id, bytes) {
  if (cache.files.has(id) || bytes.byteLength > cache.maxBytes) return;
  cache.files.set(id, bytes);
  cache.bytes += bytes.byteLength;
  for (const [old, oldBytes] of cache.files) {
    if (cache.bytes <= cache.maxBytes) break;
    cache.files.delete(old);
    cache.bytes -= oldBytes.byteLength;
  }
}

function recall(cache, id) {
  const bytes = cache?.files.get(id);
  if (bytes !== undefined) {
    cache.files.delete(id);
    cache.files.set(id, bytes);
  }
  return bytes;
}

const named = (entry, error) =>
  Object.assign(error, {
    message: `font ${JSON.stringify(entry.id)} (${entry.family}, ${entry.style}): ${error.message}`,
    id: entry.id,
    family: entry.family,
    style: entry.style,
  });

const MANIFEST_OPTIONS = ["fetch", "signal", "timeoutMs", "maxBytes", "limits", "aliases", "cache"];
// setTimeout's longest delay.
const MAX_TIMEOUT_MS = 2 ** 31 - 1;

// The package's own reading of a manifest entry, so nothing the caller
// holds is read twice.
function entryOf(entry, at) {
  const where = `entries[${at}]`;
  if (entry === null || typeof entry !== "object") throw invalid("invalid_options", `${where} must be an object`);
  const { id, family, bold, italic, url, data } = entry;
  if (typeof id !== "string" || id === "") throw invalid("invalid_options", `${where}.id must be a non-empty string`);
  if (typeof family !== "string" || family === "") throw invalid("invalid_options", `${where}.family must be a non-empty string`);
  if ((url === undefined) === (data === undefined)) throw invalid("invalid_options", `${where} needs exactly one of url and data`);
  if (data !== undefined && !(data instanceof Uint8Array)) throw invalid("invalid_options", `${where}.data must be a Uint8Array`);
  const style = bold && italic ? "bold italic" : bold ? "bold" : italic ? "italic" : "regular";
  const read = { id, family, bold: Boolean(bold), italic: Boolean(italic), style, data, url: undefined };
  if (url !== undefined) {
    if (typeof url !== "string") throw named(read, invalid("font_url_invalid", "url must be a string"));
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      throw named(read, invalid("font_url_invalid", "url is not a URL"));
    }
    if (parsed.protocol !== "https:") throw named(read, invalid("font_url_not_https", "fonts are fetched over https only"));
    // A response's url never has a fragment; keeping one would read as a redirect.
    parsed.hash = "";
    read.url = parsed.href;
  }
  return read;
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
    // Without a stream the body is read whole, then capped like a stream.
    take(new Uint8Array(await response.arrayBuffer()));
  }
  if (chunks.length === 1) return chunks[0];
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
    response = await abortable(Promise.resolve().then(() => fetching(entry.url, { signal, redirect: "error" })), signal);
  } catch (error) {
    if (signal.aborted) throw signal.reason;
    throw docrsError(`fetch failed: ${error?.message ?? error}`, { code: "io", reason: "font_fetch_failed", cause: error });
  }
  // A fetch that followed a redirect anyway.
  if (response.redirected || (response.url && response.url !== entry.url)) {
    throw invalid("font_url_redirected", "fonts are not fetched through redirects");
  }
  if (!response.ok) {
    throw docrsError(`fetch failed with HTTP ${response.status}`, { code: "io", reason: "font_fetch_failed" });
  }
  return abortable(readCapped(response, cap, spent), signal);
}

const timedOut = (timeoutMs) =>
  docrsError(`fetching fonts took longer than ${timeoutMs} ms`, { code: "io", reason: "font_fetch_timeout" });

// The files of `entries`, copied from `data`, from the cache or fetched,
// within `cap`.
async function filesOf(entries, { fetching, signal, timeoutMs, cap, cache }) {
  const controller = new AbortController();
  const timeout = timedOut(timeoutMs);
  const timer = setTimeout(() => controller.abort(timeout), timeoutMs);
  const forward = () => controller.abort(signal.reason);
  if (signal?.aborted) forward();
  signal?.addEventListener("abort", forward, { once: true });
  const spent = { bytes: 0 };
  const one = async (entry) => {
    const known = entry.data ?? recall(cache, entry.id);
    if (known === undefined) return fetchFont(entry, fetching, controller.signal, cap, spent);
    spent.bytes += known.byteLength;
    if (known.byteLength > cap.file) throw tooLarge("max_font_file_bytes", cap.file, known.byteLength);
    if (spent.bytes > cap.set) throw tooLarge("max_font_set_bytes", cap.set, spent.bytes);
    return entry.data === undefined ? known : new Uint8Array(known);
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

// Each entry's file must hold the family and style it declares; a
// variable face covers both weights.
function checkFaces(entries, faces, perFile) {
  let at = 0;
  entries.forEach((entry, file) => {
    const own = faces.slice(at, (at += perFile[file]));
    const fits = (face) =>
      face.family.toLowerCase() === entry.family.toLowerCase() &&
      face.italic === entry.italic &&
      (face.variable || face.bold === entry.bold);
    if (!own.some(fits)) {
      const held = own.map((face) => `${face.family}${face.bold ? " bold" : ""}${face.italic ? " italic" : ""}`).join(", ");
      throw named(entry, invalid("font_face_mismatch", `the file holds ${held}`));
    }
  });
}

async function loadFontManifest(entries, options = {}) {
  if (!Array.isArray(entries)) throw invalid("invalid_options", "entries must be an array");
  for (const key of Object.keys(options ?? {})) {
    if (!MANIFEST_OPTIONS.includes(key)) throw invalid("invalid_options", `unknown option ${JSON.stringify(key)}`);
  }
  const { fetch: fetching = globalThis.fetch, signal, timeoutMs = 30_000, maxBytes, limits, aliases, cache } = options ?? {};
  const read = entries.map(entryOf);
  if (!(Number.isFinite(timeoutMs) && timeoutMs > 0 && timeoutMs <= MAX_TIMEOUT_MS)) {
    throw invalid("invalid_options", `timeoutMs must be a number of milliseconds from 1 to ${MAX_TIMEOUT_MS}`);
  }
  if (maxBytes !== undefined && !(Number.isSafeInteger(maxBytes) && maxBytes >= 0)) {
    throw invalid("invalid_options", "maxBytes must be a whole number of bytes");
  }
  const store = cache === undefined || cache === null ? undefined : caches.get(cache);
  if (cache != null && store === undefined) throw invalid("invalid_options", "cache must come from createFontCache");
  const budget = fontLimits(limits);
  const cap = { file: Math.min(budget.fileBytes, maxBytes ?? Infinity), set: Math.min(budget.setBytes, maxBytes ?? Infinity) };
  const wanted = [];
  const ids = new Set();
  for (const entry of read) {
    if (!ids.has(entry.id)) wanted.push(entry);
    ids.add(entry.id);
  }
  const files = await filesOf(wanted, { fetching, signal, timeoutMs, cap, cache: store });
  let fonts;
  try {
    fonts = fontSet(files, { limits, aliases }, { owned: true, accept: (faces, perFile) => checkFaces(wanted, faces, perFile) });
  } catch (error) {
    throw Number.isInteger(error.file) ? named(wanted[error.file], error) : error;
  }
  if (store !== undefined) {
    wanted.forEach((entry, at) => {
      if (entry.url !== undefined) remember(store, entry.id, files[at]);
    });
  }
  return fonts;
}

// One assignment per name: Node reads the named exports of a CommonJS
// module from its source.
exports.createFontCache = createFontCache;
exports.fromHtml = converting("fromHtml");
exports.fromJson = converting("fromJson");
exports.fromMarkdown = converting("fromMarkdown");
exports.fromTiptap = converting("fromTiptap");
exports.inspectReference = fontless("inspectReference", 1);
exports.loadFontManifest = loadFontManifest;
exports.loadFonts = loadFonts;
exports.markdownStyles = fontless("markdownStyles", 2);
exports.referenceDocument = guarded("referenceDocument");
exports.start = guarded("start");
exports.version = guarded("version");

// For tests and diagnostics; not part of the documented interface.
Object.defineProperty(exports, "__wasm", { get: () => live().wasm });

current = instantiate();
