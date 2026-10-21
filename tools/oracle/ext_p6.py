"""Oracle extension for work package P6 (composition).

Source documents for `composition.insert_blocks_from` / `append_document`:

* {"$source": "<fixture>", "steps": [...]?} -> a docx.Document of the fixture
  (`new`, `synthetic:<name>` or an upstream fixture path), then built with
  ordinary ops steps run on it (resolved against the source). Sources are
  cached per destination document and spec, so several arguments of one
  case address the same object.
* {"$source_span": "<text>", "source": <spec>, ...} -> find_one on the source.
* {"$source_block": "<text>", "source": <spec>, "view"?, "index"?} -> a live
  source block.

`new` and synthetic fixtures are read from target/p6-sources/, written by
the Rust adapter (crates/docrs-compat/src/ops/composition.rs), which the
ops runner runs before the oracle batch.

FUNCTIONS: p6_source_unchanged(document, source) -> the source package state
equals its state right after it was built; p6_copy_letterhead(document,
source, styles=) -> upstream's private `_copy_letterhead` with a fresh report
(its tests call it directly).
"""

import importlib
import inspect
import json
import os

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.abspath(os.path.join(_HERE, "..", ".."))
_FIXTURES = os.path.join(_ROOT, "tests", "fixtures", "upstream")
_MATERIALIZED = os.path.join(_ROOT, "target", "p6-sources")

_CACHE = {"document": None, "sources": {}}


def _normalized(spec):
    if isinstance(spec, str):
        return {"$source": spec}
    normalized = {"$source": spec["$source"]}
    if "steps" in spec:
        normalized["steps"] = spec["steps"]
    return normalized


def _path(fixture):
    if fixture == "new" or fixture.startswith("synthetic:"):
        name = "new" if fixture == "new" else fixture.split(":", 1)[1]
        # Same stem as the Rust `case_safe_file_stem` (case-insensitive FS).
        stem = "".join("^" + c.lower() if "A" <= c <= "Z" else c for c in name)
        path = os.path.join(_MATERIALIZED, stem + ".docx")
        if not os.path.exists(path):
            raise RuntimeError(f"{path} was not materialized by the Rust adapter")
        return path
    return os.path.join(_FIXTURES, fixture)


def _package_state(document):
    parts = []
    for part in document.part.package.iter_parts():
        relationships = tuple(
            sorted(
                (rel.rId, rel.reltype, str(rel.target_ref), rel.is_external)
                for rel in part.rels.values()
            )
        )
        parts.append((str(part.partname), part.blob, relationships))
    return tuple(sorted(parts, key=lambda item: item[0]))


def _run_step(source, step):
    import __main__ as oracle

    module, _, name = step["op"].rpartition(".")
    if module == "ext":
        target = next(m.FUNCTIONS[name] for m in oracle._ext() if name in getattr(m, "FUNCTIONS", {}))
        args = [source] + [oracle._resolve_arg(source, a) for a in step.get("args", [])]
        kwargs = {k: oracle._resolve_arg(source, v) for k, v in step.get("kwargs", {}).items()}
        target(*args, **kwargs)
        return
    target = importlib.import_module("docx." + module)
    for attribute in name.split(":"):
        target = getattr(target, attribute)
    args = [oracle._resolve_arg(source, a) for a in step.get("args", [])]
    kwargs = {k: oracle._resolve_arg(source, v) for k, v in step.get("kwargs", {}).items()}
    parameters = list(inspect.signature(target).parameters)
    if parameters and parameters[0] == "document":
        args.insert(0, source)
    target(*args, **kwargs)


def _entry(document, spec):
    import docx

    if _CACHE["document"] is not document:
        _CACHE["document"] = document
        _CACHE["sources"] = {}
    spec = _normalized(spec)
    key = json.dumps(spec, sort_keys=True)
    entry = _CACHE["sources"].get(key)
    if entry is None:
        source = docx.Document(_path(spec["$source"]))
        for step in spec.get("steps", []):
            _run_step(source, step)
        entry = (source, _package_state(source))
        _CACHE["sources"][key] = entry
    return entry


def _source(document, value):
    return _entry(document, value)[0]


def _source_span(document, value):
    from docx.search import find_one

    source = _source(document, value["source"])
    return find_one(
        source,
        value["$source_span"],
        nth=value.get("nth"),
        match=value.get("match", "exact"),
        story=value.get("story"),
        view=value.get("view", "current"),
    )


def _source_block(document, value):
    from docx.story import iter_blocks

    source = _source(document, value["source"])
    wanted = value["$source_block"]
    matches = [b for b in iter_blocks(source, view=value.get("view", "current")) if b.text == wanted]
    index = value.get("index", 0)
    if index >= len(matches):
        raise LookupError(f"no source block with text {wanted!r}")
    return matches[index]


def p6_source_unchanged(document, source):
    for candidate, snapshot in _CACHE["sources"].values():
        if candidate is source:
            return _package_state(source) == snapshot
    raise LookupError("unknown source")


def p6_copy_letterhead(document, source, styles="match_by_name"):
    from docx.composition import CompositionReport, _copy_letterhead

    report = CompositionReport()
    _copy_letterhead(document, source, report, len(document.sections), styles)
    return report.to_dict()


RESOLVERS = {
    "$source": _source,
    "$source_span": _source_span,
    "$source_block": _source_block,
}
ENCODERS = []
FUNCTIONS = {
    "p6_source_unchanged": p6_source_unchanged,
    "p6_copy_letterhead": p6_copy_letterhead,
}
