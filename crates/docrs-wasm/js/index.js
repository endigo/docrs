"use strict";
// The wasm-bindgen glue (docrs.js) keeps its instance in a variable of its
// own with no way to replace it, and a panic leaves that instance poisoned.
// This file evaluates the glue as a function and calls it again for a
// fresh instance once the current one is poisoned.
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

function instantiate() {
  const fresh = { glue: null, wasm: null, poisoned: false };
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
  if (fresh.wasm === null || String(exported.sort()) !== String([...NAMES].sort())) {
    throw unexpected(`exports ${exported.join(", ") || "nothing"} where index.js guards ${NAMES.join(", ")}`);
  }
  fresh.glue = exports;
  return fresh;
}

let current = null;

const live = () => (current !== null && !current.poisoned ? current : (current = instantiate()));

function guarded(name) {
  const call = function (...args) {
    const used = live();
    try {
      return used.glue[name](...args);
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
  };
  Object.defineProperty(call, "name", { value: name });
  return call;
}

// One assignment per name: Node reads the named exports of a CommonJS
// module from its source.
exports.fromHtml = guarded("fromHtml");
exports.fromJson = guarded("fromJson");
exports.fromMarkdown = guarded("fromMarkdown");
exports.fromTiptap = guarded("fromTiptap");
exports.inspectReference = guarded("inspectReference");
exports.markdownStyles = guarded("markdownStyles");
exports.referenceDocument = guarded("referenceDocument");
exports.start = guarded("start");
exports.version = guarded("version");

const NAMES = Object.keys(exports);

// For tests and diagnostics; not part of the documented interface.
Object.defineProperty(exports, "__wasm", { get: () => live().wasm });

current = instantiate();
