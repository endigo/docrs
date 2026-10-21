"""Oracle extension for M4 work package p2 (fields / numbering / blocks).

RESOLVERS: {"$tag": fn(document, value)} argument decoders.
ENCODERS: [fn(value) -> (handled, encoded)] result encoders.
FUNCTIONS: {"name": fn(document, *args, **kwargs)} callable as op "ext.name".
"""


def _operand(operand):
    if operand is None:
        return None
    return {
        "value": operand.value,
        "start": operand.start,
        "end": operand.end,
        "quoted": operand.quoted,
    }


def p2_fieldcode(document, function, instruction, *args):
    """Call one `docx._fieldcode` function; operands encode as dicts."""
    from docx import _fieldcode as fc

    if function == "bookmark_operands":
        names = args[0] if args else ()
        return [_operand(o) for o in fc.bookmark_operands(instruction, implicit_names=names)]
    if function == "command_operand":
        return _operand(fc.command_operand(instruction, args[0]))
    if function == "rewrite_bookmark_operands":
        return fc.rewrite_bookmark_operands(instruction, dict(args[0]))
    if function == "rewrite_command_operand":
        return fc.rewrite_command_operand(instruction, args[0], dict(args[1]))
    raise ValueError(f"unknown fieldcode function {function!r}")


RESOLVERS = {}
ENCODERS = []
FUNCTIONS = {"p2_fieldcode": p2_fieldcode}
