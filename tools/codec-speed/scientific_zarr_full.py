#!/usr/bin/env python3
"""Store verified Parquet Float32 row-group blocks in one Zarr v3 array."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import zarr


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def inventory(path: Path) -> tuple[int, int, str]:
    digest = hashlib.sha256()
    total = 0
    files = sorted(item for item in path.rglob("*") if item.is_file())
    for item in files:
        relative = item.relative_to(path)
        size = item.stat().st_size
        total += size
        digest.update(f"{relative}\t{size}\t{sha256_file(item)}\n".encode())
    return total, len(files), digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("parquet_inventory", type=Path)
    parser.add_argument("first_manifest", type=Path)
    parser.add_argument("remaining_manifest", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("result", type=Path)
    parser.add_argument("--layout", choices=("rows", "grid"), required=True)
    parser.add_argument("--clevel", type=int, default=5)
    args = parser.parse_args()
    if args.output.exists() or args.result.exists():
        raise FileExistsError("output or result already exists")
    if not 0 <= args.clevel <= 9:
        raise ValueError("invalid compression level")
    inventory_data = json.loads(args.parquet_inventory.read_text())
    manifests = [json.loads(path.read_text()) for path in (args.first_manifest, args.remaining_manifest)]
    source_sha256 = inventory_data["source_sha256"]
    if any(manifest["source_sha256_verified_by_inventory"] != source_sha256 for manifest in manifests):
        raise ValueError("Parquet source identities differ")
    blocks = sorted((block for manifest in manifests for block in manifest["blocks"]), key=lambda x: x["row_group"])
    expected_groups = inventory_data["row_groups"]
    if [block["row_group"] for block in blocks] != list(range(len(expected_groups))):
        raise ValueError("row-group coverage is incomplete")
    rows = inventory_data["rows"]
    features = inventory_data["features"]
    chunks = (1024, features if args.layout == "rows" else min(64, features))
    array = zarr.create_array(
        store=str(args.output), shape=(rows, features), dtype="<f4", chunks=chunks,
        compressors=[zarr.codecs.BloscCodec(cname="zstd", clevel=args.clevel, shuffle="bitshuffle")],
        zarr_format=3,
    )
    offset = 0
    for block, group in zip(blocks, expected_groups, strict=True):
        if block["rows"] != group["rows"] or block["features"] != features:
            raise ValueError("row-group shape differs")
        raw = Path(block["path"])
        if sha256_file(raw) != block["sha256"] or raw.stat().st_size != block["rows"] * features * 4:
            raise ValueError("raw block hash or size differs")
        source = np.memmap(raw, dtype="<f4", mode="r", shape=(block["rows"], features))
        array[offset : offset + block["rows"], :] = source
        decoded = np.asarray(array[offset : offset + block["rows"], :])
        if not np.array_equal(decoded.view("<u4"), source.view("<u4")):
            raise ValueError(f"row group {block['row_group']} decoded Float32 bits differ")
        offset += block["rows"]
        print(json.dumps({"row_group": block["row_group"], "rows": block["rows"], "offset_after": offset, "exact": True}), flush=True)
    if offset != rows:
        raise ValueError("row count differs")
    stored_bytes, file_count, content_sha256 = inventory(args.output)
    result = {
        "format": "Zarr v3 built-in Blosc (Blosc1 wire), Zstd, bitshuffle",
        "zarr_version": zarr.__version__,
        "source_sha256": source_sha256,
        "shape": [rows, features],
        "chunks": chunks,
        "layout": args.layout,
        "clevel": args.clevel,
        "raw_bytes": rows * features * 4,
        "stored_bytes_including_metadata": stored_bytes,
        "stored_files": file_count,
        "stored_content_manifest_sha256": content_sha256,
        "exact_float32_bits": True,
        "output": str(args.output),
    }
    args.result.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
