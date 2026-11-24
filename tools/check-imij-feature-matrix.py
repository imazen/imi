#!/usr/bin/env python3
"""Check every distinct Cargo feature set of the imij library."""

import argparse
import concurrent.futures
import itertools
import json
import os
from pathlib import Path
import subprocess


def feature_sets(features):
    """Collapse combinations that Cargo resolves to the same local features."""
    names = sorted(set(features) - {"default"})
    sets = set()
    for size in range(len(names) + 1):
        for selection in itertools.combinations(names, size):
            resolved = set(selection)
            pending = list(selection)
            while pending:
                for item in features[pending.pop()]:
                    if item in names and item not in resolved:
                        resolved.add(item)
                        pending.append(item)
            sets.add(tuple(sorted(resolved)))
    return sorted(sets, key=lambda cell: (len(cell), cell))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=1, help="number of isolated Cargo targets")
    parser.add_argument("--list", action="store_true", help="write only the resolved manifest")
    parser.add_argument("--require", action="append", default=[], help="require this local feature")
    parser.add_argument("--exclude", action="append", default=[], help="exclude this local feature")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    root = Path(__file__).resolve().parents[1]
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=root
    )
    package = next(p for p in json.loads(metadata)["packages"] if p["name"] == "imij")
    cells = feature_sets(package["features"])
    required, excluded = set(args.require), set(args.exclude)
    unknown = (required | excluded) - set(package["features"])
    if unknown:
        parser.error(f"unknown features: {', '.join(sorted(unknown))}")
    selected = [
        index for index, cell in enumerate(cells) if required.issubset(cell) and excluded.isdisjoint(cell)
    ]
    if not selected:
        parser.error("feature filter selected no configurations")
    args.output.mkdir()
    out = args.output.resolve()
    (out / "cells.json").write_text(json.dumps(cells, indent=2) + "\n")
    (out / "selected.json").write_text(json.dumps(selected) + "\n")
    if args.list:
        return

    def worker(shard):
        results = []
        env = dict(os.environ, CARGO_TARGET_DIR=str(out / f"target-{shard}"), CARGO_BUILD_JOBS="1")
        for index in selected[shard::args.jobs]:
            features = ",".join(cells[index])
            command = ["cargo", "check", "--locked", "-p", "imij", "--lib", "--no-default-features"]
            if features:
                command.extend(["--features", features])
            log = out / f"cell-{index:03}.log"
            with log.open("w") as stream:
                stream.write(" ".join(command) + "\n")
                stream.flush()
                status = subprocess.run(command, cwd=root, env=env, stdout=stream,
                                        stderr=subprocess.STDOUT, check=False).returncode
            results.append((index, features, status))
            print(f"{index:03} {status} {features or '(empty)'}", flush=True)
        return results

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = sorted(itertools.chain.from_iterable(pool.map(worker, range(args.jobs))))
    (out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    raise SystemExit(1 if any(status for _, _, status in results) else 0)


if __name__ == "__main__":
    main()
