"""p7 oracle helpers: core properties (`document.core_properties`).

Mirrored by `crates/docrs-compat/src/ops/inherited/coreprops.rs`.
Dates are encoded as `YYYY-MM-DDTHH:MM:SSZ` (UTC). `now` fixes the
`modified` stamp upstream writes when it creates a default core-properties
part (`CorePropertiesPart.default` calls `dt.datetime.now(dt.timezone.utc)`).
"""

import contextlib
import datetime as dt
import types

NAMES = (
    "author",
    "category",
    "comments",
    "content_status",
    "created",
    "identifier",
    "keywords",
    "language",
    "last_modified_by",
    "last_printed",
    "modified",
    "revision",
    "subject",
    "title",
    "version",
)


def _encode(value):
    if isinstance(value, dt.datetime):
        return "%04d-%02d-%02dT%02d:%02d:%02dZ" % (
            value.year,
            value.month,
            value.day,
            value.hour,
            value.minute,
            value.second,
        )
    return value


@contextlib.contextmanager
def _fixed_now(now):
    import docx.opc.parts.coreprops as module

    if now is None:
        yield
        return

    class _Fixed(dt.datetime):
        @classmethod
        def now(cls, tz=None):  # noqa: D401 - stub
            return now

    original = module.dt
    module.dt = types.SimpleNamespace(datetime=_Fixed, timezone=dt.timezone)
    try:
        yield
    finally:
        module.dt = original


def _props(core_properties):
    return {name: _encode(getattr(core_properties, name)) for name in NAMES}


def p7_core_properties(document, now=None):
    with _fixed_now(now):
        core_properties = document.core_properties
    return _props(core_properties)


def p7_set_core_properties(document, changes, now=None):
    """Assign `[[name, value], ...]` in order; return the new values."""
    with _fixed_now(now):
        core_properties = document.core_properties
    for name, value in changes:
        if name not in NAMES:
            raise LookupError("unknown core property %r" % name)
        setattr(core_properties, name, value)
    return _props(core_properties)
