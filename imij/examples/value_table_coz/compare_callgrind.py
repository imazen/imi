#!/usr/bin/env python3
"""Compare the JSON lines from the cg_decode benchmark's main/change logs.

Whole-image instruction counts must stay within +/-0.05%; stream and reader
counts may increase at most 0.5%. A failed gate exits 1 and prints every case.
"""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("main", type=Path)
parser.add_argument("change", type=Path)
args = parser.parse_args()

def read(path):
    cases = {}
    for line in path.read_text().splitlines():
        if line.startswith("{"):
            case = json.loads(line)
            key = case["function_name"], case["id"]
            assert key not in cases, f"duplicate case: {key}"
            cases[key] = case["profiles"][0]["data"]["total"]["metrics"]["Ir"]["values"]["new"]
    return cases

main = read(args.main)
change = read(args.change)
assert main.keys() == change.keys() and len(main) == 21, "incomplete cg_decode suite"
failed = []
for key in sorted(main):
    ratio = change[key] / main[key]
    assert key[0] in ("decode", "stream", "reader"), key
    ok = abs(ratio - 1) <= 0.0005 if key[0] == "decode" else ratio <= 1.005
    print(*key, main[key], change[key], f"{ratio:.8f}", "PASS" if ok else "FAIL")
    if not ok:
        failed.append((key, ratio))
print("FAILING", failed)
raise SystemExit(bool(failed))
