#!/usr/bin/env python3
"""Validate original KADID pair metadata and PNG headers before a storage study.

This reads headers, not pixels. The codec runner must hash full inputs, decode
with zenpng, and verify every stored raster before reporting compression results.
"""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import zlib

SOURCE = "https://database.mmsp-kn.de/kadid-10k-database.html"
PAPER = "https://doi.org/10.1109/QoMEX.2019.8743252"


def png_header(path):
    with path.open("rb") as stream:
        head = stream.read(33)
    if (len(head) != 33 or head[:8] != b"\x89PNG\r\n\x1a\n"
            or head[8:16] != b"\x00\x00\x00\rIHDR"
            or zlib.crc32(head[12:29]) != int.from_bytes(head[29:33], "big")):
        raise ValueError(f"invalid PNG IHDR: {path}")
    width, height, depth, color, compression, filtering, interlace = struct.unpack(
        ">IIBBBBB", head[16:29])
    if (not width or not height or depth != 8 or color != 2 or compression != 0
            or filtering != 0 or interlace not in (0, 1)):
        raise ValueError(f"expected original RGB8 PNG: {path}")
    return {"width": width, "height": height, "encoded_bytes": path.stat().st_size,
            "header_sha256": hashlib.sha256(head).hexdigest()}


def prepare(root, output):
    table = root / "dmos.csv"
    raw_table = table.read_bytes()
    with table.open(newline="") as stream:
        reader = csv.DictReader(stream)
        if reader.fieldnames != ["dist_img", "ref_img", "dmos", "var"]:
            raise ValueError(f"unexpected original score columns: {reader.fieldnames}")
        rows = list(reader)
    if len(rows) != 10125:
        raise ValueError(f"incomplete KADID table: {len(rows)} pairs")
    groups = {}
    seen = set()
    for row in rows:
        dist, ref = row["dist_img"], row["ref_img"]
        match = re.fullmatch(r"I(\d{2})_(\d{2})_(\d{2})\.png", dist)
        if not match or ref != f"I{match[1]}.png" or dist in seen:
            raise ValueError(f"invalid or duplicate pair: {row}")
        if not 1 <= int(match[2]) <= 25 or not 1 <= int(match[3]) <= 5:
            raise ValueError(f"unknown distortion/level: {dist}")
        score, variance = float(row["dmos"]), float(row["var"])
        if not math.isfinite(score) or not math.isfinite(variance) or variance < 0:
            raise ValueError(f"invalid score/variance: {dist}")
        seen.add(dist)
        groups.setdefault(ref, []).append({
            "filename": dist, "reference_filename": ref,
            # Original decimal strings, never the derived training-score transform.
            "dmos": row["dmos"], "variance": row["var"],
            "distortion_type": int(match[2]), "distortion_level": int(match[3]),
        })
    expected_refs = {f"I{i:02}.png" for i in range(1, 82)}
    if set(groups) != expected_refs or any(len(g) != 125 for g in groups.values()):
        raise ValueError("incomplete reference groups")
    records = []
    for ref, distortions in sorted(groups.items()):
        reference = png_header(root / "images" / ref)
        items = [{"filename": ref, "reference_filename": ref, "is_reference": True,
                  **reference}]
        for item in sorted(distortions, key=lambda d: d["filename"]):
            header = png_header(root / "images" / item["filename"])
            if (header["width"], header["height"]) != (reference["width"], reference["height"]):
                raise ValueError(f"reference dimensions differ: {item['filename']}")
            items.append({**item, "is_reference": False, **header})
        records.append({"reference": ref, "items": items})
    # Exclusive output: a partial or prior preparation never gets overwritten.
    output.mkdir(parents=False, exist_ok=False)
    (output / "dmos.csv").write_bytes(raw_table)
    manifest = {
        "dataset": "KADID-10k", "source_url": SOURCE, "paper_url": PAPER,
        "source_root": str(root.resolve()), "table_sha256": hashlib.sha256(raw_table).hexdigest(),
        "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "preparation_scope": "pair metadata and PNG IHDR only; no raster/compression claims",
        "resized": False, "references": len(records), "distortions": len(rows),
        "original_png_bytes": sum(i["encoded_bytes"] for g in records for i in g["items"]),
        "score_table_bytes": len(raw_table), "groups": records,
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    summary = {k: v for k, v in manifest.items() if k != "groups"}
    summary["manifest_sha256"] = hashlib.sha256((output / "manifest.json").read_bytes()).hexdigest()
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dataset", type=Path)
    parser.add_argument("new_output", type=Path)
    args = parser.parse_args()
    prepare(args.dataset, args.new_output)
