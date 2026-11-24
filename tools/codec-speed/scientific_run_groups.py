#!/usr/bin/env python3
"""Independently verify and encode Parquet Float32 row groups with IMIJ."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def run_logged(command: list[str], log: Path) -> None:
    with log.open("xb") as stream:
        result = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=False)
    if result.returncode:
        raise RuntimeError(f"command exited {result.returncode}; see {log}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("imij_program", type=Path)
    parser.add_argument("verify_program", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--expected-source-sha256", required=True)
    parser.add_argument("--features", type=int, required=True)
    parser.add_argument("--effort", type=float, required=True)
    parser.add_argument("--threads", type=int, default=1)
    args = parser.parse_args()
    if args.output.exists():
        raise FileExistsError(args.output)
    manifest = json.loads(args.manifest.read_text())
    if manifest["source_sha256_verified_by_inventory"] != args.expected_source_sha256:
        raise ValueError("manifest source identity differs")
    if sha256_file(args.source) != args.expected_source_sha256:
        raise ValueError("Parquet source bytes differ")
    args.output.mkdir(parents=True)
    results = []
    for block in manifest["blocks"]:
        index = block["row_group"]
        raw = Path(block["path"])
        if block["features"] != args.features or sha256_file(raw) != block["sha256"]:
            raise ValueError(f"row group {index} raw block changed")
        verify_result = args.output / f"verify-rg{index:02}.json"
        run_logged(
            [
                "python3", str(args.verify_program), str(args.source), str(raw),
                str(verify_result), "--row-group", str(index), "--features",
                str(args.features), "--expected-block-sha256", block["sha256"],
            ],
            args.output / f"verify-rg{index:02}.log",
        )
        verified = json.loads(verify_result.read_text())
        if verified["mismatches"] or verified["checked_float32_bit_patterns"] != block["rows"] * args.features:
            raise ValueError(f"row group {index} was not fully checked")
        encoded = args.output / f"features-rg{index:02}.imi"
        run_logged(
            [
                str(args.imij_program), str(raw), str(args.features), str(block["rows"]),
                str(args.effort), str(args.threads), str(encoded),
            ],
            args.output / f"encode-rg{index:02}.log",
        )
        if "exact=true" not in (args.output / f"encode-rg{index:02}.log").read_text():
            raise ValueError(f"row group {index} lacks exact IMIJ decode confirmation")
        entry = {
            "row_group": index,
            "rows": block["rows"],
            "raw_bytes": block["bytes"],
            "raw_sha256": block["sha256"],
            "encoded_bytes": encoded.stat().st_size,
            "encoded_sha256": sha256_file(encoded),
            "verified_float32_bit_patterns": verified["checked_float32_bit_patterns"],
            "imij_exact_decode": True,
        }
        results.append(entry)
        print(json.dumps(entry), flush=True)
    if sha256_file(args.source) != args.expected_source_sha256:
        raise ValueError("Parquet source changed during comparison")
    summary = {
        "source_sha256": args.expected_source_sha256,
        "program_sha256": sha256_file(args.imij_program),
        "effort": args.effort,
        "threads": args.threads,
        "groups": results,
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    main()
