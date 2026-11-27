"""Usage: python3 -I tools/pdf/checks/test_compare.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import compare  # noqa: E402


def para(text, page, y=72.0, row_end=False):
    return {"text": text, "page": page, "y": y, "row_end": row_end, "heading": False}


def row_end(page, y):
    return para(None, page, y, row_end=True)


def word(*pages):
    return {"pages": [{"page": i + 1, "lines": [{"text": t} for t in lines]} for i, lines in enumerate(pages)]}


def ours(*paragraphs, pages=2):
    return {"pages": pages, "paragraphs": list(paragraphs)}


class Compare(unittest.TestCase):
    def test_a_heading_is_not_matched_to_its_contents_entry(self):
        result = compare.compare(
            ours(para("Contents", 1), para("Summary\t2", 1), para("Summary", 2), para("Revenue grew this year.", 2)),
            word(["Contents", "Summary ............................ 2"], ["Summary", "Revenue grew this year."]),
        )
        self.assertEqual((result["paragraphs"], result["agree"], result["contents"]), (3, 3, 1))

    def test_table_cells_split_into_lines_are_found(self):
        result = compare.compare(
            ours(
                para("Region", 1, 150), para("Q1", 1, 150), para("Q2", 1, 150), row_end(1, 150),
                para("North America", 1, 163), para("1,200", 1, 163), para("1,420", 1, 163), row_end(1, 163),
                pages=1,
            ),
            word(["Region", "Q1 Q2", "North America 1,200 1,420"]),
        )
        self.assertEqual((result["paragraphs"], result["agree"]), (4, 4))
        self.assertEqual((result["table cell"], result["not found"]), (2, 0))

    def test_a_list_label_on_its_own_line_is_skipped(self):
        result = compare.compare(
            ours(para("Confirm the Europe forecast.", 1), para("Circulate the review by Friday.", 2)),
            word(["1.", "Confirm the Europe forecast."], ["2.", "Circulate the review", "by Friday."]),
        )
        self.assertEqual((result["paragraphs"], result["agree"]), (2, 2))

    def test_matches_follow_document_order(self):
        result = compare.compare(
            ours(para("Next steps follow.", 2), para("Appendix", 2)),
            word(["Appendix"], ["Next steps follow.", "Appendix"]),
        )
        self.assertEqual((result["paragraphs"], result["agree"]), (2, 2))

    def test_unmatched_paragraphs_have_reasons(self):
        result = compare.compare(
            ours(para("", 1), para("Hi", 1), para("Nowhere in Word at all", 1), pages=1),
            word(["Something else entirely"]),
        )
        self.assertEqual((result["empty"], result["short"], result["not found"]), (1, 1, 1))

    def test_a_document_with_nothing_compared_scores_nothing(self):
        result = compare.compare(ours(para("", 1), pages=1), word([]))
        self.assertEqual(compare.share(result), "n/a")
        lines = compare.summary([("empty", result)])
        self.assertIn("- Documents: 1, 1 with no paragraph compared", lines)
        self.assertIn("- Documents with every compared paragraph on Word's page: 0", lines)

    def test_a_document_mostly_not_found_is_warned_about(self):
        result = compare.compare(
            ours(para("Present in Word", 1), para("Missing paragraph one", 1), para("Missing paragraph two", 1), pages=1),
            word(["Present in Word"]),
        )
        self.assertTrue(any(line.startswith("- Warning") for line in compare.summary([("doc", result)])))


if __name__ == "__main__":
    unittest.main()
