#!/usr/bin/env python3
"""Estimate a complete JXL tar from matched, completed maximum-profile groups."""

import argparse
import hashlib
import json
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def journal(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines()]


def tar_member_bytes(payload: int) -> int:
    return 512 + ((payload + 511) // 512) * 512


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fast_root", type=Path)
    parser.add_argument("max_root", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--fast-archive-bytes", type=int, required=True)
    parser.add_argument("--expected-images", type=int, required=True)
    parser.add_argument("--images-per-group", type=int, required=True)
    args = parser.parse_args()

    fast_summary = json.loads((args.fast_root / "summary.json").read_text())
    fast_journal = args.fast_root / "completed.jsonl"
    assert sha256(fast_journal) == fast_summary["journal_sha256"]
    fast_rows = journal(fast_journal)
    fast = {row["filename"]: row for row in fast_rows}
    assert len(fast) == len(fast_rows) == args.expected_images
    assert sum(row["jxl_bytes"] for row in fast_rows) == fast_summary["jxl_payload_bytes"]

    matched: dict[str, dict] = {}
    groups = []
    for group in sorted((args.max_root / "groups").iterdir()):
        summary_path = group / "summary.json"
        if not summary_path.is_file():
            continue  # An interrupted group cannot contribute to this estimate.
        summary = json.loads(summary_path.read_text())
        journal_path = group / "completed.jsonl"
        assert sha256(journal_path) == summary["journal_sha256"]
        assert summary["images"] == args.images_per_group
        assert summary["profile"] == "max"
        assert summary["encoder_sha256"] == fast_summary["encoder_sha256"]
        assert summary["source_manifest_sha256"] == fast_summary["source_manifest_sha256"]
        rows = journal(journal_path)
        assert len(rows) == args.images_per_group
        assert len({row["filename"] for row in rows}) == len(rows)
        assert sum(row["jxl_bytes"] for row in rows) == summary["jxl_payload_bytes"]
        fast_bytes = 0
        for row in rows:
            filename = row["filename"]
            assert row["group"] == group.name
            assert filename not in matched and filename in fast
            assert row["source_sha256"] == fast[filename]["source_sha256"]
            image = group / (Path(filename).stem + ".jxl")
            assert image.stat().st_size == row["jxl_bytes"]
            assert sha256(image) == row["jxl_sha256"]
            matched[filename] = row
            fast_bytes += fast[filename]["jxl_bytes"]
        groups.append(
            {
                "group": group.name,
                "images": len(rows),
                "fast_bytes": fast_bytes,
                "max_bytes": summary["jxl_payload_bytes"],
                "max_to_fast_ratio": summary["jxl_payload_bytes"] / fast_bytes,
                "summary_sha256": sha256(summary_path),
            }
        )
    assert groups
    sample_fast = sum(group["fast_bytes"] for group in groups)
    sample_max = sum(group["max_bytes"] for group in groups)
    ratio = sample_max / sample_fast
    fixed_tar_bytes = args.fast_archive_bytes - sum(
        tar_member_bytes(row["jxl_bytes"]) for row in fast_rows
    )
    assert fixed_tar_bytes > 0
    projected_sizes = {
        filename: matched[filename]["jxl_bytes"]
        if filename in matched
        else round(row["jxl_bytes"] * ratio)
        for filename, row in fast.items()
    }
    result = {
        "method": "byte-weighted matched complete-group ratio; observed max sizes retained, remaining sizes scaled from each fast file; USTAR 512-byte padding projected per file",
        "complete_groups": groups,
        "sample_images": len(matched),
        "total_images": len(fast),
        "sample_fast_bytes": sample_fast,
        "sample_max_bytes": sample_max,
        "sample_max_to_fast_ratio": ratio,
        "observed_group_ratio_min": min(group["max_to_fast_ratio"] for group in groups),
        "observed_group_ratio_max": max(group["max_to_fast_ratio"] for group in groups),
        "fast_payload_bytes_exact": fast_summary["jxl_payload_bytes"],
        "fast_archive_bytes_exact": args.fast_archive_bytes,
        "fixed_metadata_tar_bytes_exact": fixed_tar_bytes,
        "max_payload_bytes_estimated": sum(projected_sizes.values()),
        "max_archive_bytes_estimated": fixed_tar_bytes
        + sum(tar_member_bytes(size) for size in projected_sizes.values()),
        "fast_journal_sha256": sha256(fast_journal),
        "caveat": "The completed groups are the first eight by input order, not a random sample. This is an extrapolation, not a measured full maximum archive or a confidence interval.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
