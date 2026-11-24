#!/usr/bin/env python3
"""Measure SafeTensors plus whole-file Zstd on verified Float32 row groups."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import safetensors
import safetensors.numpy
import zstandard


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("parquet_inventory", type=Path)
    parser.add_argument("first_manifest", type=Path)
    parser.add_argument("remaining_manifest", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--level", type=int, default=5)
    args = parser.parse_args()
    if args.output.exists():
        raise FileExistsError(args.output)
    if not 1 <= args.level <= 19:
        raise ValueError("Zstd level is outside the measured range")
    inventory = json.loads(args.parquet_inventory.read_text())
    manifests = [json.loads(path.read_text()) for path in (args.first_manifest, args.remaining_manifest)]
    if any(manifest["source_sha256_verified_by_inventory"] != inventory["source_sha256"] for manifest in manifests):
        raise ValueError("Parquet source identities differ")
    blocks = sorted((block for manifest in manifests for block in manifest["blocks"]), key=lambda x: x["row_group"])
    if [block["row_group"] for block in blocks] != list(range(len(inventory["row_groups"]))):
        raise ValueError("row-group coverage is incomplete")
    args.output.mkdir(parents=True)
    entries = []
    compressor = zstandard.ZstdCompressor(level=args.level, threads=1)
    decompressor = zstandard.ZstdDecompressor()
    for block in blocks:
        raw = Path(block["path"])
        rows = block["rows"]
        features = block["features"]
        if sha256_file(raw) != block["sha256"] or raw.stat().st_size != rows * features * 4:
            raise ValueError(f"row group {block['row_group']} raw bits changed")
        source = np.memmap(raw, dtype="<f4", mode="r", shape=(rows, features))
        serialized = safetensors.numpy.save({"features": source})
        compressed = compressor.compress(serialized)
        path = args.output / f"features-rg{block['row_group']:02}.safetensors.zst"
        with path.open("xb") as stream:
            stream.write(compressed)
        readback = path.read_bytes()
        decoded_file = decompressor.decompress(readback, max_output_size=len(serialized))
        decoded = safetensors.numpy.load(decoded_file)["features"]
        if decoded.dtype != np.dtype("<f4") or decoded.shape != (rows, features):
            raise ValueError("SafeTensors decoded shape or type differs")
        if not np.array_equal(source.view("<u4"), decoded.view("<u4")):
            raise ValueError(f"row group {block['row_group']} Float32 bits differ")
        entry = {
            "row_group": block["row_group"],
            "rows": rows,
            "features": features,
            "raw_sha256": block["sha256"],
            "safetensors_bytes_before_zstd": len(serialized),
            "stored_bytes": len(readback),
            "stored_sha256": sha256_file(path),
            "exact_float32_bits": True,
        }
        entries.append(entry)
        print(json.dumps(entry), flush=True)
    result = {
        "format": "one SafeTensors F32 matrix per Parquet row group, whole-file Zstd",
        "safetensors_version": safetensors.__version__,
        "zstandard_version": zstandard.__version__,
        "zstd_level": args.level,
        "zstd_threads": 1,
        "source_sha256": inventory["source_sha256"],
        "rows": sum(entry["rows"] for entry in entries),
        "features": inventory["features"],
        "stored_bytes": sum(entry["stored_bytes"] for entry in entries),
        "random_access_note": "Zstd wraps each entire SafeTensors file; columns within one file require decompressing that file.",
        "groups": entries,
    }
    (args.output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: result[key] for key in ("rows", "features", "stored_bytes", "zstd_level")}), flush=True)


if __name__ == "__main__":
    main()
