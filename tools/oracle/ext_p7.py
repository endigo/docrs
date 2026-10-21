"""Oracle extension for work package p7 (M7: the remaining inherited
python-docx public API).

RESOLVERS: {"$tag": fn(document, value)} argument decoders.
ENCODERS: [fn(value) -> (handled, encoded)] result encoders.
FUNCTIONS: {"name": fn(document, *args, **kwargs)} callable as op "ext.name".

The helpers are split by area into `tools/oracle/p7_*.py` (sections,
coreprops, styles, text, tables) plus the shared locators in
`p7_common.py`; this module loads them and merges their tables. Every
`p7_*` function is mirrored by a Rust adapter of the same name in
`crates/docrs-compat/src/ops/inherited/`.
"""

import glob
import importlib.util
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, os.path.join(_HERE, name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_load("p7_common")

RESOLVERS = {}
ENCODERS = []
FUNCTIONS = {}

for _path in sorted(glob.glob(os.path.join(_HERE, "p7_*.py"))):
    _module = _load(os.path.basename(_path)[:-3])
    RESOLVERS.update(getattr(_module, "RESOLVERS", {}))
    ENCODERS.extend(getattr(_module, "ENCODERS", []))
    FUNCTIONS.update(
        {
            name: fn
            for name, fn in vars(_module).items()
            if name.startswith("p7_") and callable(fn)
        }
    )
