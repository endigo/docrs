"""p7 oracle helpers: style writing (`document.styles`, `BaseStyle` and
subclass setters, `LatentStyles`, `_LatentStyle`).

Mirrored by `crates/docrs-compat/src/ops/inherited/styles.rs`. Styles
are addressed by `styles[key]` (UI name, else style id); latent styles by
`latent_styles[key]`. Style types use their XML tokens ("paragraph",
"character", "table", "numbering").
"""

import p7_common

_STYLE_ORDER = (
    "name",
    "style_id",
    "hidden",
    "locked",
    "priority",
    "quick_style",
    "unhide_when_used",
    "base_style",
    "next_paragraph_style",
)

_LATENT_DEFAULTS = (
    "default_priority",
    "load_count",
    "default_to_hidden",
    "default_to_locked",
    "default_to_quick_style",
    "default_to_unhide_when_used",
)

_LATENT_PROPS = ("hidden", "locked", "priority", "quick_style", "unhide_when_used")


def _summary(style):
    from docx.enum.style import WD_STYLE_TYPE

    out = {
        "name": style.name,
        "style_id": style.style_id,
        "type": style.type.xml_value,
        "builtin": style.builtin,
        "hidden": style.hidden,
        "locked": style.locked,
        "priority": style.priority,
        "quick_style": style.quick_style,
        "unhide_when_used": style.unhide_when_used,
    }
    if style.type != WD_STYLE_TYPE.LIST:
        base = style.base_style
        out["base_style"] = None if base is None else base.style_id
    if style.type in (WD_STYLE_TYPE.PARAGRAPH, WD_STYLE_TYPE.TABLE):
        out["next_paragraph_style"] = style.next_paragraph_style.style_id
    return out


def _style_type(token):
    from docx.enum.style import WD_STYLE_TYPE

    return WD_STYLE_TYPE.from_xml(token)


def p7_styles(document):
    """`len(styles)` and every style's (name, style_id) in order."""
    styles = document.styles
    return {
        "count": len(styles),
        "styles": [[style.name, style.style_id] for style in styles],
    }


def p7_style(document, key):
    return p7_common.error_class(lambda: _summary(document.styles[key]))


def p7_style_contains(document, name):
    return name in document.styles


def p7_add_style(document, name, style_type, builtin=False):
    return _summary(document.styles.add_style(name, _style_type(style_type), builtin))


def p7_delete_style(document, key):
    def run():
        styles = document.styles
        styles[key].delete()
        return len(styles)

    return p7_common.error_class(run)


def p7_style_set(document, key, props):
    """Assign `props` ({name: value}) in canonical order; `base_style` and
    `next_paragraph_style` values are style keys (or None)."""
    styles = document.styles
    unknown = set(props) - set(_STYLE_ORDER)
    if unknown:
        raise LookupError("unknown style properties %r" % sorted(unknown))
    style = styles[key]
    for prop in _STYLE_ORDER:
        if prop not in props:
            continue
        value = props[prop]
        if prop in ("base_style", "next_paragraph_style") and value is not None:
            value = styles[value]
        setattr(style, prop, value)
    return _summary(style)


def p7_style_font(document, key):
    return p7_common.error_class(lambda: p7_common.font_values(document.styles[key].font))


def p7_style_set_font(document, key, changes):
    def run():
        font = document.styles[key].font
        p7_common.apply_font(font, changes)
        return p7_common.font_values(font)

    return p7_common.error_class(run)


def p7_style_parfmt(document, key):
    return p7_common.error_class(
        lambda: p7_common.parfmt_values(document.styles[key].paragraph_format)
    )


def p7_style_set_parfmt(document, key, props):
    def run():
        fmt = document.styles[key].paragraph_format
        p7_common.apply_parfmt(fmt, props)
        return p7_common.parfmt_values(fmt)

    return p7_common.error_class(run)


def _latent_summary(latent_style):
    return {
        "name": latent_style.name,
        "hidden": latent_style.hidden,
        "locked": latent_style.locked,
        "priority": latent_style.priority,
        "quick_style": latent_style.quick_style,
        "unhide_when_used": latent_style.unhide_when_used,
    }


def p7_latent_styles(document):
    latent = document.styles.latent_styles
    out = {prop: getattr(latent, prop) for prop in _LATENT_DEFAULTS}
    out["count"] = len(latent)
    out["names"] = [latent_style.name for latent_style in latent]
    return out


def p7_latent_style(document, key):
    return p7_common.error_class(lambda: _latent_summary(document.styles.latent_styles[key]))


def p7_latent_set(document, props):
    latent = document.styles.latent_styles
    unknown = set(props) - set(_LATENT_DEFAULTS)
    if unknown:
        raise LookupError("unknown latent defaults %r" % sorted(unknown))
    for prop in _LATENT_DEFAULTS:
        if prop in props:
            setattr(latent, prop, props[prop])
    return p7_latent_styles(document)


def p7_latent_add(document, name):
    return _latent_summary(document.styles.latent_styles.add_latent_style(name))


def p7_latent_delete(document, key):
    def run():
        latent = document.styles.latent_styles
        latent[key].delete()
        return len(latent)

    return p7_common.error_class(run)


def p7_latent_style_set(document, key, props):
    def run():
        latent_style = document.styles.latent_styles[key]
        for prop in _LATENT_PROPS:
            if prop in props:
                setattr(latent_style, prop, props[prop])
        return _latent_summary(latent_style)

    return p7_common.error_class(run)
