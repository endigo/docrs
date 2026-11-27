"""Usage: python3 -I tools/pdf/compare.py DIR (tools/pdf/README.md)."""

import json
import os
import re
import sys

NEEDLE = 24
CELL_NEEDLE = 12
SHORTEST = 4
WARN_NOT_FOUND = 0.2
LEADER = re.compile(r"(\.\s*){4,}\d+\s*$|…\s*\d+\s*$")
CONTENTS_ENTRY = re.compile(r"\t\d+$")
SKIPPED = ("contents", "empty", "short", "table cell")


def letters(text):
    return "".join(ch for ch in text.lower() if ch.isalnum())


def word_stream(word):
    """Word's text as letters and digits, with the page of each; contents lines left out."""
    text, pages = [], []
    for page in word["pages"]:
        for line in page["lines"]:
            if LEADER.search(line["text"]):
                continue
            chars = letters(line["text"])
            text.append(chars)
            pages.extend([page["page"]] * len(chars))
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
    text = paragraph.get("text") or ""
    if CONTENTS_ENTRY.search(text):
        return "contents", None
    needle = letters(text)
    if not needle:
        return "empty", None
    if len(needle) < SHORTEST:
        return ("table cell" if in_cell else "short"), None
    return None, needle[: CELL_NEEDLE if in_cell else NEEDLE]


def compare(ours, word):
    stream, pages = word_stream(word)
    paragraphs = ours["paragraphs"]
    cells = table_cells(paragraphs)
    result = {"paragraphs": 0, "agree": 0, "not found": 0, "first_miss": ""}
    result.update({reason: 0 for reason in SKIPPED})
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
            result["not found"] += 1
            continue
        cursor = at + len(needle)
        result["paragraphs"] += 1
        if pages[at] == paragraph["page"]:
            result["agree"] += 1
        elif not result["first_miss"]:
            result["first_miss"] = f'"{paragraph["text"]}" ours {paragraph["page"]}, Word {pages[at]}'
    result["pages_ours"] = ours["pages"]
    result["pages_word"] = len(word["pages"])
    return result


def share(row):
    return f"{row['agree'] / row['paragraphs']:.4f}" if row["paragraphs"] else "n/a"


def not_found_share(row):
    looked_for = row["paragraphs"] + row["not found"]
    return row["not found"] / looked_for if looked_for else 0.0


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
    columns = ["pages_ours", "pages_word", "paragraphs", "agree", "not found", *SKIPPED, "first_miss"]
    with open(path, "w") as handle:
        handle.write("case\t" + "\t".join(c.replace(" ", "_") for c in columns) + "\tshare\n")
        for case, row in rows:
            handle.write(case + "\t" + "\t".join(str(row[c]) for c in columns) + f"\t{share(row)}\n")


def summary(rows):
    compared = [row for _, row in rows if row["paragraphs"]]
    total = sum(row["paragraphs"] for row in compared)
    agree = sum(row["agree"] for row in compared)
    lines = [
        "# Paragraph start pages: docrs layout against Word",
        "",
        f"- Documents: {len(rows)}, {len(rows) - len(compared)} with no paragraph compared",
        f"- Paragraphs compared: {total}"
        + (f"; on Word's page: {agree} ({100 * agree / total:.1f}%)" if total else ""),
        f"- Documents with every compared paragraph on Word's page: "
        f"{sum(1 for row in compared if row['agree'] == row['paragraphs'])}",
        f"- Documents with Word's page count: {sum(1 for _, row in rows if row['pages_ours'] == row['pages_word'])}",
        "- Not compared: "
        + ", ".join(f"{reason} {sum(row[reason] for _, row in rows)}" for reason in ("not found", *SKIPPED)),
    ]
    warned = [case for case, row in rows if not_found_share(row) > WARN_NOT_FOUND]
    if warned:
        lines.append(f"- Warning: over {WARN_NOT_FOUND:.0%} of paragraphs not found in Word's text: {', '.join(warned)}")
    lines += ["", "| Document | Pages (ours/Word) | Agree | Not found | First difference |", "| -- | -- | -- | -- | -- |"]
    for case, row in rows:
        agreed = f"{row['agree']}/{row['paragraphs']}" if row["paragraphs"] else "n/a"
        miss = row["first_miss"].replace("|", "\\|")
        lines.append(f"| {case} | {row['pages_ours']}/{row['pages_word']} | {agreed} | {row['not found']} | {miss} |")
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
