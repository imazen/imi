#!/usr/bin/env python3
"""Summarize a saved Cargo/run-heavy gate without discarding its raw log."""
import argparse
import json
from pathlib import Path
import re


def summarize(source, path):
    lines = path.read_text().splitlines()
    targets = []
    current = None
    for line in lines:
        match = re.search(r"Running (.+) \((.+)\)", line)
        if match:
            current = {"target": match[1], "binary": match[2]}
        result = re.match(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", line)
        if result:
            if current is None:
                raise ValueError("test result has no executable identity")
            current.update(status=result[1], passed=int(result[2]), failed=int(result[3]), ignored=int(result[4]))
            targets.append(current)
            current = None
    receipts = [line for line in lines if line.startswith("run-heavy: done rc=")]
    if not receipts:
        raise ValueError("gate has no completion receipt")
    return {
        "source": source, "raw_log": str(path.resolve()), "receipt": receipts[-1],
        "targets": targets,
        "totals": {key: sum(target[key] for target in targets) for key in ("passed", "failed", "ignored")},
        "executables": len(targets),
        "failing_executables": sum(target["failed"] != 0 for target in targets),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source")
    parser.add_argument("log", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    result = summarize(args.source, args.log)
    with args.output.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps({key: result[key] for key in ("source", "receipt", "totals", "executables", "failing_executables")}))
