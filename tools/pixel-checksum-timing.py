#!/usr/bin/env python3
"""Run an already built probe under the caller's host lock/resource wrapper."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import statistics
import subprocess

p = argparse.ArgumentParser()
p.add_argument("binary", type=Path)
p.add_argument("files", type=Path)
p.add_argument("output", type=Path)
p.add_argument("--source", required=True)
p.add_argument("--rounds", type=int, default=11)
a = p.parse_args()
a.output.mkdir()
binary = a.binary.resolve()
metadata = {"source": a.source, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "host": platform.node(), "platform": platform.platform(), "paired_process_rounds": a.rounds,
            "observations_per_process": 1, "decode_warmups_per_process": 3,
            "shuffle_seed": 731, "commands": []}
observations = {}
paired = {}

def run(name, process, args):
    stem = f"{name}-r{process}"
    command = ["/usr/bin/time", "-v", "-o", str(a.output / (stem + ".rss.txt")), str(binary)] + args
    metadata["commands"].append(command)
    with (a.output / (stem + ".tsv")).open("w") as out:
        subprocess.run(command, stdout=out, check=True)
    with (a.output / (stem + ".tsv")).open() as inp:
        rows = list(csv.DictReader(inp, delimiter="\t"))
    assert len(rows) == 1, "one measured observation per fresh process"
    result = {}
    for key in rows[0]:
        if key.endswith("_ns"):
            value = int(rows[0][key])
            observations.setdefault((name, key), []).append(value)
            result[key] = value
    (a.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(stem, "PASS", flush=True)
    return result

with (a.files / "files.tsv").open() as inp:
    cases = list(csv.DictReader(inp, delimiter="\t"))
cells = [(case, threads) for case in cases for threads in [1, 2, 4, 8]]
cell_ordinals = {(case["name"], threads): i for i, (case, threads) in enumerate(cells)}
rng = random.Random(metadata["shuffle_seed"])
for process in range(a.rounds):
    rng.shuffle(cells)
    for case, threads in cells:
        # Alternate the first state for each cell/round, while shuffling cells.
        states = ["off", "on"] if (process + cell_ordinals[(case["name"], threads)]) % 2 == 0 else ["on", "off"]
        pair = {}
        for state in states:
            name = f'{case["name"]}-{state}-t{threads}'
            pair[state] = run(name, process, ["time", str((a.files / (case["name"] + "-" + state + ".imi")).resolve()),
                                             str(threads), "1"])
        for metric in pair["off"]:
            paired.setdefault((case["name"], threads, metric), []).append(pair["on"][metric] / pair["off"][metric])
        name = f'{case["name"]}-encode-t{threads}'
        run(name, process, ["encode-time", case["kind"], case["width"], str(threads), "1"])
with (a.output / "summary.tsv").open("w") as out:
    writer = csv.writer(out, delimiter="\t")
    writer.writerow(["case", "metric", "processes", "observations_per_process", "median_ns", "p90_ns", "worst_ns"])
    for (name, metric), values in sorted(observations.items()):
        values.sort()
        writer.writerow([name, metric, len(values), 1, statistics.median(values),
                         values[math.ceil(.9 * len(values)) - 1], values[-1]])
with (a.output / "paired.tsv").open("w") as out:
    writer = csv.writer(out, delimiter="\t")
    writer.writerow(["case", "threads", "metric", "process_pairs", "median_on_off", "p90_on_off", "worst_on_off"])
    for (name, threads, metric), values in sorted(paired.items()):
        values.sort()
        writer.writerow([name, threads, metric, len(values), statistics.median(values),
                         values[math.ceil(.9 * len(values)) - 1], values[-1]])
