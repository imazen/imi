#!/usr/bin/env python3
"""Exercise archived and full-width seed-evaluation schemas through the CLI."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

REPORT = Path(__file__).resolve().parents[1] / "benchmarks/entropy_seeds_2026-10-07/report.py"


class SeedReportSchema(unittest.TestCase):
    def report(self, rows):
        with tempfile.TemporaryDirectory(dir=os.environ["TMPDIR"]) as directory:
            root = Path(directory)
            split = root / "split.tsv"
            split.write_text("rendition\tlongest\texposed\tclass\nfixture\t96\tno\tschema-only\n")
            data = ("\n".join("\t".join(row) for row in rows) + "\n").encode()
            packed = subprocess.run(["zstd", "-q", "-c"], input=data, capture_output=True, check=True)
            (root / "rows.tsv.zst").write_bytes(packed.stdout)
            return subprocess.run([sys.executable, str(REPORT), str(split), str(root / "aggregate.tsv"), str(root)], capture_output=True, text=True)

    def test_archived_and_full_width_rows_with_and_without_times(self):
        base = ["fixture", "E", "xyb", "rl", "0", "5", "100", "90", "0", "0"]
        old = base + ["17", "1", "-"]
        new = base + [str(2**64 - 1), "2", "1", "-"]
        result = self.report([old, old + ["10", "20"], new, new + ["11", "21"]])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("4 cells; seeded decodes not bit-identical: 0; files larger with seeds: 0", result.stdout)
        self.assertIn("AC 2 (50.0%), DC 2 (50.0%), alpha 0", result.stdout)

    def test_unknown_new_scope_is_refused(self):
        row = ["fixture", "E", "xyb", "rl", "0", "5", "100", "90", "0", "0", "128", "8", "1", "-"]
        result = self.report([row])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid bank/scope selection", result.stderr)


if __name__ == "__main__":
    unittest.main()
