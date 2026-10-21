"""Oracle extension for M4 work package p3 (tableops, formatting, styles).

RESOLVERS: {"$tag": fn(document, value)} argument decoders.
ENCODERS: [fn(value) -> (handled, encoded)] result encoders.
FUNCTIONS: {"name": fn(document, *args, **kwargs)} callable as op "ext.name".

The style helpers report python-docx's read API (`document.styles`,
`Styles[...]`, `get_by_id`, `get_style_id`, `default`, latent styles and
`BabelFish`) as plain JSON so the Rust snapshot can be compared field by
field. Reading latent styles never calls `get_or_add_latentStyles`.
"""

import warnings


def _run(document, value):
    from docx.search import find_one
    from docx.oxml.ns import qn
    from docx.text.paragraph import Paragraph
    from docx.text.run import Run

    span = find_one(
        document,
        value["$run"],
        nth=value.get("nth"),
        match=value.get("match", "exact"),
        story=value.get("story"),
        view=value.get("view", "current"),
    )
    run = span._atoms[0].run
    paragraph = run.getparent()
    while paragraph is not None and paragraph.tag != qn("w:p"):
        paragraph = paragraph.getparent()
    return Run(run, Paragraph(paragraph, document._body))


RESOLVERS = {"$run": _run}


def _table_grid(table):
    return [[cell.text for cell in row.cells] for row in table.rows]


def _encode_table(value):
    from docx.table import Table

    if isinstance(value, Table):
        return True, {"type": "Table", "rows": _table_grid(value)}
    return False, None


ENCODERS = [_encode_table]


def _type_token(style_type):
    return None if style_type is None else style_type.xml_value


def _style_summary(style):
    if style is None:
        return None
    summary = {
        "style_id": style.style_id,
        "name": style.name,
        "type": _type_token(style.type),
        "builtin": style.builtin,
        "hidden": style.hidden,
        "locked": style.locked,
        "priority": style.priority,
        "quick_style": style.quick_style,
        "unhide_when_used": style.unhide_when_used,
        "base_style": None,
        "next_paragraph_style": None,
    }
    base = getattr(style, "base_style", None)
    if base is not None:
        summary["base_style"] = base.style_id
    if hasattr(style, "next_paragraph_style"):
        summary["next_paragraph_style"] = style.next_paragraph_style.style_id
    return summary


def _wd(token):
    from docx.enum.style import WD_STYLE_TYPE

    return WD_STYLE_TYPE.from_xml(token)


def p3_styles(document):
    from docx.enum.style import WD_STYLE_TYPE

    styles = document.styles
    defaults = {}
    for member in (
        WD_STYLE_TYPE.PARAGRAPH,
        WD_STYLE_TYPE.CHARACTER,
        WD_STYLE_TYPE.TABLE,
        WD_STYLE_TYPE.LIST,
    ):
        default = styles.default(member)
        defaults[member.xml_value] = None if default is None else default.style_id
    return {
        "count": len(styles),
        "styles": [_style_summary(style) for style in styles],
        "defaults": defaults,
        "latent": p3_latent(document),
    }


def p3_style(document, key):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            return _style_summary(document.styles[key])
        except KeyError:
            return None


def p3_style_contains(document, name):
    return name in document.styles


def p3_style_default(document, style_type):
    return _style_summary(document.styles.default(_wd(style_type)))


def p3_style_get_by_id(document, style_id, style_type):
    return _style_summary(document.styles.get_by_id(style_id, _wd(style_type)))


def p3_style_get_style_id(document, name, style_type):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            return document.styles.get_style_id(name, _wd(style_type))
        except KeyError as exc:  # the Rust API reports an unknown name as invalid_argument
            raise ValueError(str(exc)) from exc


def _latent_summary(latent):
    return {
        "name": latent.name,
        "hidden": latent.hidden,
        "locked": latent.locked,
        "priority": latent.priority,
        "quick_style": latent.quick_style,
        "unhide_when_used": latent.unhide_when_used,
    }


def _latent_styles(document):
    from docx.styles.latent import LatentStyles

    element = document.styles.element.latentStyles
    return None if element is None else LatentStyles(element)


def p3_latent(document):
    latent = _latent_styles(document)
    if latent is None:
        return None
    return {
        "count": len(latent),
        "default_priority": latent.default_priority,
        "default_to_hidden": latent.default_to_hidden,
        "default_to_locked": latent.default_to_locked,
        "default_to_quick_style": latent.default_to_quick_style,
        "default_to_unhide_when_used": latent.default_to_unhide_when_used,
        "load_count": latent.load_count,
        "exceptions": [_latent_summary(item) for item in latent],
    }


def p3_latent_style(document, name):
    latent = _latent_styles(document)
    if latent is None:
        return None
    try:
        return _latent_summary(latent[name])
    except KeyError:
        return None


def p3_babel(document, names):
    from docx.styles import BabelFish

    return [
        {
            "name": name,
            "ui2internal": BabelFish.ui2internal(name),
            "internal2ui": BabelFish.internal2ui(name),
        }
        for name in names
    ]


def p3_table_grid(document, table):
    return _table_grid(table)


def p3_format_value(document, target, key):
    from docx.formatting import format_of

    return format_of(target).value_of(key)


def _error_code(exc):
    from docx import errors

    for cls, code in (
        (errors.AmbiguousTargetError, "ambiguous_target"),
        (errors.TargetNotFoundError, "target_not_found"),
        (errors.UnsupportedStructureError, "unsupported_structure"),
        (errors.BoundaryViolationError, "boundary_violation"),
    ):
        if isinstance(exc, cls):
            return code
    if isinstance(exc, (TypeError, ValueError)):
        return "invalid_argument"
    raise exc


def p3_format_blocks(document):
    """format_of every paragraph block (all stories, current view)."""
    from docx.formatting import format_of
    from docx.story import iter_blocks
    from docx.text.paragraph import Paragraph

    out = []
    for block in iter_blocks(document):
        if block.kind != "paragraph":
            continue
        try:
            out.append(format_of(Paragraph(block._element, document._body)).to_dict())
        except Exception as exc:  # noqa: BLE001
            out.append({"error": _error_code(exc)})
    return out


def p3_format_spans(document):
    """format_of the span of every paragraph text found exactly once."""
    from docx.formatting import format_of
    from docx.search import find_text
    from docx.story import iter_blocks

    out = []
    for block in iter_blocks(document):
        if block.kind != "paragraph" or not block.text:
            continue
        spans = find_text(document, block.text)
        if len(spans) != 1:
            continue
        try:
            out.append({"text": block.text, "format": format_of(spans[0]).to_dict()})
        except Exception as exc:  # noqa: BLE001
            out.append({"text": block.text, "error": _error_code(exc)})
    return out


FUNCTIONS = {
    "p3_format_blocks": p3_format_blocks,
    "p3_format_spans": p3_format_spans,
    "p3_styles": p3_styles,
    "p3_style": p3_style,
    "p3_style_contains": p3_style_contains,
    "p3_style_default": p3_style_default,
    "p3_style_get_by_id": p3_style_get_by_id,
    "p3_style_get_style_id": p3_style_get_style_id,
    "p3_latent": p3_latent,
    "p3_latent_style": p3_latent_style,
    "p3_babel": p3_babel,
    "p3_table_grid": p3_table_grid,
    "p3_format_value": p3_format_value,
}
