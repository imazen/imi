#!/usr/bin/env python3
"""Exercise matching with an unrelated fixture phrase."""
import xxhash
import unittest
from check_repository_terms import restricted_lines


class NamingPolicyTests(unittest.TestCase):
    rules = {9: {xxhash.xxh3_128(b'paperkite').hexdigest()}}

    def matches(self, text):
        return list(restricted_lines(text.splitlines(), self.rules))

    def test_case_separators_and_identifier_prefix(self):
        for text in ('Paper Kite', 'paper-kite', 'PAPER_KITE', 'zenpaperkite', 'paper\t_kite'):
            self.assertEqual(self.matches(text), [1])

    def test_name_across_lines(self):
        self.assertEqual(self.matches('prefix paper\n\nkite suffix'), [3])

    def test_unicode_normalization(self):
        self.assertEqual(self.matches('ｐａｐｅｒ ＫＩＴＥ'), [1])

    def test_partial_and_unrelated_input(self):
        self.assertEqual(self.matches('paper kit\nother text'), [])
        self.assertEqual(self.matches('unrelated phrase'), [])


if __name__ == '__main__':
    unittest.main()
