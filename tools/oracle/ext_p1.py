"""Oracle extension for M4 work package p1 (controls, bookmarks, links, notes).

RESOLVERS: {"$tag": fn(document, value)} argument decoders.
ENCODERS: [fn(value) -> (handled, encoded)] result encoders.
FUNCTIONS: {"name": fn(document, *args, **kwargs)} callable as op "ext.name".
"""

import datetime as dt


def _day(document, value):
    return dt.date.fromisoformat(value["$day"])


RESOLVERS = {"$day": _day}
ENCODERS = []


def _select(document, index=None, tag=None, alias=None):
    from docx.controls import get_control, iter_controls

    if index is not None:
        return list(iter_controls(document))[index]
    return get_control(document, tag=tag, alias=alias)


def p1_control_state(document, index=None, tag=None, alias=None):
    control = _select(document, index, tag, alias)
    return {
        "tag": control.tag,
        "alias": control.alias,
        "control_type": control.control_type,
        "is_data_bound": control.is_data_bound,
        "showing_placeholder": control.showing_placeholder,
        "value": control.value,
        "info": control.info().to_dict(),
    }


def p1_control_set_value(document, value, index=None, tag=None, alias=None):
    _select(document, index, tag, alias).set_value(value)


def p1_control_set_twice(document, first, second, index=None, tag=None, alias=None):
    control = _select(document, index, tag, alias)
    control.set_value(first)
    control.set_value(second)


def p1_bookmark_then_footnote(document, span, name, text):
    from docx.bookmarks import create_bookmark
    from docx.notes import add_footnote

    info = create_bookmark(document, span, name)
    return [info.to_dict(), add_footnote(document, span, text)]


def p1_link_stale_span(document, span, replacement, address):
    from docx.links import add_hyperlink
    from docx.search import find_one

    find_one(document, span.text).replace(replacement)
    link = add_hyperlink(document, span, address)
    return {"type": "Hyperlink", "address": link.address, "text": link.text}


def p1_stacked_notes(document, span, pairs):
    from docx.notes import add_endnote, add_footnote

    ids = []
    for kind, text in pairs:
        ids.append((add_endnote if kind == "endnote" else add_footnote)(document, span, text))
    return ids


FUNCTIONS = {
    "p1_control_state": p1_control_state,
    "p1_control_set_value": p1_control_set_value,
    "p1_control_set_twice": p1_control_set_twice,
    "p1_bookmark_then_footnote": p1_bookmark_then_footnote,
    "p1_link_stale_span": p1_link_stale_span,
    "p1_stacked_notes": p1_stacked_notes,
}
