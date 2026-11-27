"""Usage: python3 -I tools/pdf/compare.py DIR (tools/pdf/README.md)."""

import json
import os
import re
import sys

NEEDLE = 24
CELL_NEEDLE = 12
SHORTEST = 4
WARN_MISSED = 0.2
LEADER = re.compile(r"(\.\s*){4,}\d+\s*$|…\s*\d+\s*$")
CONTENTS_ENTRY = re.compile(r"\t\d+$")
SKIPPED = ("contents", "empty", "short", "table_cell")
MISSED = ("not_found", "passed_over")


def letters(text):
    return "".join(ch for ch in text.lower() if ch.isalnum())


def is_contents(paragraph):
    if "toc" in paragraph:
        return paragraph["toc"]
    return bool(CONTENTS_ENTRY.search(paragraph.get("text") or ""))


def word_lines(word):
    return [(page["page"], line["text"], letters(line["text"])) for page in word["pages"] for line in page["lines"]]


def starts(line, needle):
    return line.startswith(needle) or (len(line) >= SHORTEST and needle.startswith(line))


def contents_block(lines, paragraphs):
    """Word's lines from the contents title to the first heading after the contents, as a range."""
    contents = [letters(p["text"])[:NEEDLE] for p in paragraphs if not p["row_end"] and is_contents(p)]
    after = next((i for i, p in enumerate(paragraphs) if not p["row_end"] and is_contents(p)), None)
    if after is None:
        return range(0)
    heading = next((letters(p["text"])[:NEEDLE] for p in paragraphs[after:]
                    if p.get("heading") and not is_contents(p) and letters(p["text"] or "")), None)
    start = next((i for i, line in enumerate(lines) if contents[0] and starts(line[2], contents[0])), None)
    if start is None or heading is None:
        return range(0)
    entries = sum(1 for entry in contents if entry.startswith(heading) or heading.startswith(entry))
    seen = 0
    for i in range(start, len(lines)):
        if starts(lines[i][2], heading):
            seen += 1
            if seen > entries:
                return range(start, i)
    return range(0)


def word_stream(lines, block):
    text, pages = [], []
    for i, (page, raw, chars) in enumerate(lines):
        if i in block or LEADER.search(raw):
            continue
        text.append(chars)
        pages.extend([page] * len(chars))
    return "".join(text), pages


def table_cells(paragraphs):
    """Indices of paragraphs in table cells: those before a row end, on its page, from its top down."""
    cells = set()
    for end, row in enumerate(paragraphs):
        if not row["row_end"]:
            continue
        i = end - 1
        while i >= 0 and (paragraphs[i]["row_end"] or (
                paragraphs[i]["page"] == row["page"] and paragraphs[i]["y"] >= row["y"])):
            cells.add(i)
            i -= 1
    return cells


def classify(paragraph, in_cell):
    if is_contents(paragraph):
        return "contents", None
    needle = letters(paragraph.get("text") or "")
    if not needle:
        return "empty", None
    if len(needle) < SHORTEST:
        return ("table_cell" if in_cell else "short"), None
    return None, needle[: CELL_NEEDLE if in_cell else NEEDLE]


def compare(ours, word):
    paragraphs = ours["paragraphs"]
    lines = word_lines(word)
    stream, pages = word_stream(lines, contents_block(lines, paragraphs))
    cells = table_cells(paragraphs)
    result = {key: 0 for key in ("compared", "agree", *MISSED, *SKIPPED)}
    result["first_miss"] = ""
    cursor = 0
    for index, paragraph in enumerate(paragraphs):
        if paragraph["row_end"]:
            continue
        reason, needle = classify(paragraph, index in cells)
        if reason:
            result[reason] += 1
            continue
        at = stream.find(needle, cursor)
        if at < 0:
            result["passed_over" if needle in stream[:cursor] else "not_found"] += 1
            continue
        cursor = at + len(needle)
        result["compared"] += 1
        if pages[at] == paragraph["page"]:
            result["agree"] += 1
        elif not result["first_miss"]:
            result["first_miss"] = f'"{paragraph["text"]}" ours {paragraph["page"]}, Word {pages[at]}'
    result["pages_ours"] = ours["pages"]
    result["pages_word"] = len(word["pages"])
    return result


def share(row):
    return f"{row['agree'] / row['compared']:.4f}" if row["compared"] else "n/a"


def missed_share(row):
    missed = sum(row[key] for key in MISSED)
    looked_for = row["compared"] + missed
    return missed / looked_for if looked_for else 0.0


def load(directory):
    rows = []
    for name in sorted(os.listdir(directory)):
        if not name.endswith(".layout.json"):
            continue
        case = name[: -len(".layout.json")]
        lines = os.path.join(directory, case + ".word.lines.json")
        if not os.path.exists(lines):
            continue
        with open(os.path.join(directory, name)) as handle:
            ours = json.load(handle)
        with open(lines) as handle:
            word = json.load(handle)
        rows.append((case, compare(ours, word)))
    return rows


def write_tsv(path, rows):
    columns = ["pages_ours", "pages_word", "compared", "agree", *MISSED, *SKIPPED, "first_miss"]
    with open(path, "w") as handle:
        handle.write("case\t" + "\t".join(columns) + "\tshare\n")
        for case, row in rows:
            handle.write(case + "\t" + "\t".join(str(row[c]) for c in columns) + f"\t{share(row)}\n")


def summary(rows):
    compared = [row for _, row in rows if row["compared"]]
    total = sum(row["compared"] for row in compared)
    agree = sum(row["agree"] for row in compared)
    lines = [
        "# Paragraph start pages: docrs layout against Word",
        "",
        f"- Documents: {len(rows)}, {len(rows) - len(compared)} with no paragraph compared",
        f"- Paragraphs compared: {total}"
        + (f"; on Word's page: {agree} ({100 * agree / total:.1f}%)" if total else ""),
        f"- Documents with every compared paragraph on Word's page: "
        f"{sum(1 for row in compared if row['agree'] == row['compared'])}",
        f"- Documents with Word's page count: {sum(1 for _, row in rows if row['pages_ours'] == row['pages_word'])}",
        "- Not compared: "
        + ", ".join(f"{key} {sum(row[key] for _, row in rows)}" for key in (*MISSED, *SKIPPED)),
    ]
    warned = [case for case, row in rows if missed_share(row) > WARN_MISSED]
    if warned:
        lines.append(f"- Warning: over {WARN_MISSED:.0%} of paragraphs not found or passed over: {', '.join(warned)}")
    lines += [
        "",
        "| Document | Pages (ours/Word) | Agree | Not found | Passed over | First difference |",
        "| -- | -- | -- | -- | -- | -- |",
    ]
    for case, row in rows:
        agreed = f"{row['agree']}/{row['compared']}" if row["compared"] else "n/a"
        miss = row["first_miss"].replace("|", "\\|")
        lines.append(
            f"| {case} | {row['pages_ours']}/{row['pages_word']} | {agreed} | {row['not_found']} "
            f"| {row['passed_over']} | {miss} |"
        )
    return lines


def main():
    directory = sys.argv[1]
    rows = load(directory)
    write_tsv(os.path.join(directory, "summary.tsv"), rows)
    lines = summary(rows)
    with open(os.path.join(directory, "summary.md"), "w") as handle:
        handle.write("\n".join(lines) + "\n")
    print("\n".join(line for line in lines[2:] if line.startswith("- ")))


if __name__ == "__main__":
    main()
