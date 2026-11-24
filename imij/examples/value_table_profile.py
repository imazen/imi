#!/usr/bin/env python3
"""Paired value-table decode study using lowbit_eval's exact pixel oracle.

Run under the usual resource wrapper. Arguments are DIRECTORY OUTPUT.tsv
ARM=BINARY ...; DIRECTORY/sel.txt lists the image IDs. Each process checks its
warmup and every timed decode against ID.raw. The output records every trial;
summaries must take the maximum median time over rounds per arm/image/thread.

Use --synthetic for value_table_sizes output, --off-only for a layout study,
and --rounds 24 for the evaluation standard's four-thread control. Build each
source arm and linker layout in its own fresh Cargo target directory: copied
source timestamps and a shared target can otherwise reuse the wrong artifact.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("arms", nargs="+", help="ARM=BINARY")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--reps", type=int, default=7)
    parser.add_argument("--off-only", action="store_true")
    parser.add_argument("--synthetic", action="store_true")
    parser.add_argument("--host-lock", type=Path, required=True)
    parser.add_argument("--work-lock", type=Path)
    parser.add_argument("--agent", default="value-table-study")
    args = parser.parse_args()
    if args.rounds < 1 or args.reps < 1:
        parser.error("rounds and reps must be positive")
    arms = [arm.split("=", 1) for arm in args.arms]
    if any(len(arm) != 2 for arm in arms):
        parser.error("each arm must be ARM=BINARY")
    if len({arm for arm, _ in arms}) != len(arms):
        parser.error("arm names must be unique")
    ids = (
        [f"k{k}" for k in [256, 1024, 4096, 16384, 32768, 49152]]
        if args.synthetic
        else (args.directory / "sel.txt").read_text().splitlines()
    )
    if not ids or any(not image_id for image_id in ids):
        parser.error("the file selection must be nonempty")
    modes = ["off"] if args.off_only else ["off", "table" if args.synthetic else "auto"]
    if args.output.exists():
        parser.error("output already exists; preserve the earlier study")
    metadata = {
        "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "arguments": {key: str(value) if isinstance(value, Path) else value
                      for key, value in vars(args).items()},
        "host": os.uname().nodename,
        "binary_sha256": {
            arm: hashlib.sha256(Path(binary).read_bytes()).hexdigest()
            for arm, binary in arms
        },
        "environment": {key: os.environ[key] for key in ["IMIJ_NO_AVX512", "RUSTFLAGS"]
                        if key in os.environ},
    }
    with args.output.with_suffix(".meta.json").open("x") as out:
        json.dump(metadata, out, indent=2)
    with args.output.open("x", buffering=1) as out:
        for round_id in range(1, args.rounds + 1):
            while os.getloadavg()[0] > 2:
                print("WAIT load", os.getloadavg(), flush=True)
                time.sleep(15)
            print("ROUND", round_id, "LOAD", os.getloadavg(), flush=True)
            for image_index, image_id in enumerate(ids):
                if args.work_lock:
                    args.work_lock.write_text(
                        time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
                        + f" {args.agent} timing {args.output.name}: "
                        + f"round {round_id}, image {image_index + 1}/{len(ids)}\n"
                    )
                for threads in [1, 2, 4, 8]:
                    for mode in modes:
                        order = arms[:]
                        random.Random(round_id * 10000 + image_index * 100 + threads).shuffle(order)
                        for arm, binary in order:
                            command = [
                                "flock", "--close", str(args.host_lock), binary, "time",
                                str(args.directory / f"{image_id}.{mode}.imi"),
                                str(args.directory / f"{image_id}.raw"),
                                str(threads), str(args.reps),
                            ]
                            result = subprocess.run(command, capture_output=True, text=True)
                            if result.returncode:
                                failure = {
                                    "command": command, "returncode": result.returncode,
                                    "stdout": result.stdout, "stderr": result.stderr,
                                }
                                with args.output.with_suffix(".failure.json").open("x") as error:
                                    json.dump(failure, error, indent=2)
                                raise RuntimeError(f"decode failed: {command}\n{result.stderr}")
                            if len(result.stdout.strip().split("\t")) != 5:
                                raise RuntimeError(f"unexpected timing output: {result.stdout!r}")
                            out.write(f"{round_id}\t{image_id}\t{mode}\t{arm}\t"
                                      + result.stdout.strip() + "\n")
                print(f"PROGRESS round={round_id} image={image_index + 1}/{len(ids)}", flush=True)
        out.write("DONE\n")


if __name__ == "__main__":
    main()
