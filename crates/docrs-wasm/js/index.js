"use strict";
// Entry point of the package: the functions of the wasm-bindgen glue
// (docrs.js), each behind a guard that replaces the WebAssembly instance
// after a panic.
//
// The module is built to abort on a panic (a bug in docrs-wasm). The call
// traps, which JavaScript sees as a `WebAssembly.RuntimeError`, and the
// instance is poisoned from then on: what the call had allocated is never
// freed, its stack pointer is wherever the panic left it, and after a few
// hundred panics the instance dies with "memory access out of bounds". Any
// other exception that unwinds through the module's frames (a stack
// overflow, say) leaves the stack pointer behind the same way. The glue
// keeps its instance in a variable of its own with no way to replace it,
// so this file evaluates the glue as a function and calls it again once
// the instance is poisoned: a fresh instance of the module compiled below
// (compiled once, the file never read again), fresh glue state and imports
// bound to it, and the panic hook set again by `start`. Nothing here refers
// to the old instance afterwards: it is garbage, with whatever the panic
// left behind, for the next full collection.
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

/** The instance's exports with every function marking `state` poisoned
 *  when it throws: whatever the exception, it unwound the module's frames. */
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

/** The glue evaluated once more, around a fresh instance: its exports, the
 *  instance's marking exports and the mark. */
function instantiate() {
  const fresh = { glue: null, wasm: null, poisoned: false };
  // What the glue sees as `fs` and `WebAssembly`: the module file is not
  // read again (`new WebAssembly.Module` hands back the module compiled
  // above whatever it is given) and the instance the glue creates carries
  // the marking exports. Everything else is the real thing.
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

/** The glue in use: a fresh one when there is none yet or the last one is poisoned. */
const live = () => (current !== null && !current.poisoned ? current : (current = instantiate()));

/** The glue's function of that name, run on the instance in use. When the
 *  call leaves that instance poisoned, a fresh one takes its place and the
 *  exception is rethrown as it is. No retry: a panic is deterministic. */
function guarded(name) {
  const call = function (...args) {
    const used = live();
    try {
      return used.glue[name](...args);
    } catch (error) {
      if (used.poisoned) {
        // V8 keeps the frames of a stack trace, the instance they ran in
        // among them, until the trace is formatted. Format it now so that
        // the error, which the caller keeps or logs, holds no instance.
        void error.stack;
        if (current === used) {
          // Let go of the poisoned instance first: should a fresh one fail
          // to come up, the next call tries again instead of running on it.
          current = null;
          try {
            live();
          } catch {
            // Reported by the next call, which instantiates again.
          }
        }
      }
      throw error;
    }
  };
  Object.defineProperty(call, "name", { value: name });
  return call;
}

// One assignment per name: Node finds the named exports of a CommonJS
// module in its source, so that `import { fromHtml } from "docrs-wasm"`
// and `const { fromHtml } = await import("docrs-wasm")` work.
exports.fromHtml = guarded("fromHtml");
exports.fromJson = guarded("fromJson");
exports.fromMarkdown = guarded("fromMarkdown");
exports.fromTiptap = guarded("fromTiptap");
exports.inspectReference = guarded("inspectReference");
exports.markdownStyles = guarded("markdownStyles");
exports.referenceDocument = guarded("referenceDocument");
exports.start = guarded("start");
exports.version = guarded("version");

/** The names above; the glue must export exactly these functions. */
const NAMES = Object.keys(exports);

// The exports of the instance in use (its `memory` among them), for tests
// and diagnostics; a call through them that throws marks the instance like
// one through the functions above. Not part of the documented interface.
Object.defineProperty(exports, "__wasm", { get: () => live().wasm });

current = instantiate();
