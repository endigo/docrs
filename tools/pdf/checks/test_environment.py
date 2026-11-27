"""Usage: python3 -I tools/pdf/checks/test_environment.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import environment  # noqa: E402

HOME = os.path.expanduser("~")


class HomeRelative(unittest.TestCase):
    def test_paths_under_home_start_with_a_tilde(self):
        self.assertEqual(environment.home_relative(HOME + "/Library/Fonts/Inter.ttf"), "~/Library/Fonts/Inter.ttf")
        self.assertEqual(environment.home_relative(HOME), "~")

    def test_other_paths_are_kept(self):
        self.assertEqual(environment.home_relative("/System/Library/Fonts/Courier.ttc"), "/System/Library/Fonts/Courier.ttc")
        self.assertEqual(environment.home_relative(HOME + "x/Fonts/a.ttf"), HOME + "x/Fonts/a.ttf")


if __name__ == "__main__":
    unittest.main()
