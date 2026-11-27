"""Usage: python3 -I tools/pdf/compare.py DIR (tools/pdf/README.md)."""

import json
import os
import sys

NEEDLE = 24


def letters(text):
    return "".join(ch for ch in text.lower() if ch.isalnum())


def compare(ours, word):
    stream, pages = [], []
    for page in word["pages"]:
        for line in page["lines"]:
            text = letters(line["text"])
            stream.append(text)
            pages.extend([page["page"]] * len(text))
    stream = "".join(stream)
    cursor = 0
    counted = agree = unmatched = 0
    first_miss = None
    for paragraph in ours["paragraphs"]:
        if paragraph["row_end"] or not paragraph.get("text"):
            continue
        needle = letters(paragraph["text"])[:NEEDLE]
        if len(needle) < 4:
            continue
        at = stream.find(needle, cursor)
        if at < 0:
            unmatched += 1
            continue
        cursor = at + len(needle)
        counted += 1
        if pages[at] == paragraph["page"]:
            agree += 1
        elif first_miss is None:
            first_miss = f'"{paragraph["text"]}" ours {paragraph["page"]}, Word {pages[at]}'
    return {
        "pages_ours": ours["pages"],
        "pages_word": len(word["pages"]),
        "paragraphs": counted,
        "agree": agree,
        "unmatched": unmatched,
        "first_miss": first_miss or "",
    }


def main():
    directory = sys.argv[1]
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
    columns = ["pages_ours", "pages_word", "paragraphs", "agree", "unmatched", "first_miss"]
    with open(os.path.join(directory, "summary.tsv"), "w") as handle:
        handle.write("case\t" + "\t".join(columns) + "\tshare\n")
        for case, row in rows:
            share = row["agree"] / row["paragraphs"] if row["paragraphs"] else 1.0
            handle.write(case + "\t" + "\t".join(str(row[c]) for c in columns) + f"\t{share:.4f}\n")
    total = sum(row["paragraphs"] for _, row in rows)
    agree = sum(row["agree"] for _, row in rows)
    exact = sum(1 for _, row in rows if row["agree"] == row["paragraphs"])
    same_pages = sum(1 for _, row in rows if row["pages_ours"] == row["pages_word"])
    summary = [
        "# Paragraph start pages: docrs layout against Word",
        "",
        f"- Documents: {len(rows)}",
        f"- Paragraphs compared: {total}"
        + (f"; on Word's page: {agree} ({100 * agree / total:.1f}%)" if total else ""),
        f"- Documents with every paragraph on Word's page: {exact}",
        f"- Documents with Word's page count: {same_pages}",
        f"- Paragraphs not found in Word's text: {sum(row['unmatched'] for _, row in rows)}",
        "",
        "| Document | Pages (ours/Word) | Agree | First difference |",
        "| -- | -- | -- | -- |",
    ]
    for case, row in rows:
        miss = row["first_miss"].replace("|", "\\|")
        summary.append(
            f"| {case} | {row['pages_ours']}/{row['pages_word']} | {row['agree']}/{row['paragraphs']} | {miss} |"
        )
    with open(os.path.join(directory, "summary.md"), "w") as handle:
        handle.write("\n".join(summary) + "\n")
    print("\n".join(summary[2:7]))


if __name__ == "__main__":
    main()
