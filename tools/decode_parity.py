#!/usr/bin/env python3
"""Seal frozen codec fixtures and execute identical inputs across decoder backends."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import subprocess
import time


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def fixture_hashes(directory):
    manifest = directory / "fixtures.tsv"
    paths = [manifest]
    for line in manifest.read_text().splitlines()[1:]:
        name = line.split("\t")[0]
        if not name or Path(name).name != name:
            raise ValueError("invalid fixture name")
        paths.extend(directory / (name + suffix) for suffix in (".imi", ".raw"))
    return {p.name: digest(p) for p in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    seal = sub.add_parser("seal", help="record hashes of an independently generated fixture set")
    seal.add_argument("fixtures", type=Path)
    seal.add_argument("--generator", type=Path, required=True)
    seal.add_argument("--provenance", required=True, help="source snapshot and reference description")
    check = sub.add_parser("check", help="verify hashes and run a native or emulated decoder")
    check.add_argument("fixtures", type=Path)
    check.add_argument("--binary", type=Path, required=True)
    check.add_argument("--runner", default="", help="command prefix, e.g. wasmtime --dir /path")
    check.add_argument("--backend", choices=("native", "scalar", "avx2"), default="native")
    check.add_argument("--threads", type=int, default=1)
    check.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    directory = args.fixtures.resolve()
    hashes = fixture_hashes(directory)
    manifest = directory / "sha256.json"
    if args.action == "seal":
        with manifest.open("x") as out:
            json.dump({"files": hashes, "generator_sha256": digest(args.generator),
                       "provenance": args.provenance}, out, indent=2)
            out.write("\n")
        print(f"sealed {(len(hashes)-1)//2} fixtures")
        return
    if args.threads < 1:
        raise ValueError("threads must be positive")
    if hashes != json.loads(manifest.read_text())["files"]:
        raise ValueError("fixture data changed after sealing")
    if args.output.exists():
        raise ValueError("use a new result path")
    env = dict(os.environ)
    for key in ("IMIJ_SCALAR", "IMIJ_NO_AVX512"):
        env.pop(key, None)
    if args.backend == "scalar":
        env["IMIJ_SCALAR"] = "1"
    elif args.backend == "avx2":
        env["IMIJ_NO_AVX512"] = "1"
    command = shlex.split(args.runner) + [str(args.binary.resolve()), "check", str(directory), str(args.threads)]
    start = time.monotonic()
    result = subprocess.run(command, env=env, text=True, capture_output=True)
    record = {"command": command, "requested_backend": args.backend, "threads": args.threads,
              "host": platform.node(), "platform": platform.platform(),
              "binary_sha256": digest(args.binary), "fixture_manifest_sha256": digest(manifest),
              "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr,
              "validation_seconds_not_a_benchmark": time.monotonic()-start}
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(result.stdout, end="")
    if result.returncode:
        print(result.stderr, end="")
        raise SystemExit(result.returncode)
    if fixture_hashes(directory) != hashes:
        raise ValueError("fixture data changed during check")


if __name__ == "__main__":
    main()
