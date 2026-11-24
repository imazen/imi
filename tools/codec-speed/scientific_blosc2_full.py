#!/usr/bin/env python3
"""Store verified Parquet Float32 blocks in one Blosc2 N-D array."""

import argparse
import hashlib
import json
from pathlib import Path

import blosc2
import numpy as np


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
    parser.add_argument("result", type=Path)
    parser.add_argument("--layout", choices=("rows", "grid"), required=True)
    parser.add_argument("--clevel", type=int, default=5)
    args = parser.parse_args()
    if args.output.exists() or args.result.exists():
        raise FileExistsError("output or result already exists")
    if not 0 <= args.clevel <= 9:
        raise ValueError("invalid compression level")
    inventory = json.loads(args.parquet_inventory.read_text())
    manifests = [json.loads(path.read_text()) for path in (args.first_manifest, args.remaining_manifest)]
    if any(manifest["source_sha256_verified_by_inventory"] != inventory["source_sha256"] for manifest in manifests):
        raise ValueError("Parquet source identities differ")
    blocks = sorted((block for manifest in manifests for block in manifest["blocks"]), key=lambda x: x["row_group"])
    if [block["row_group"] for block in blocks] != list(range(len(inventory["row_groups"]))):
        raise ValueError("row-group coverage is incomplete")
    rows = inventory["rows"]
    features = inventory["features"]
    chunks = (1024, features if args.layout == "rows" else min(64, features))
    blocks_shape = (128, min(64, chunks[1]))
    array = blosc2.empty(
        (rows, features), dtype="<f4", chunks=chunks, blocks=blocks_shape,
        cparams={
            "codec": blosc2.Codec.ZSTD,
            "clevel": args.clevel,
            "filters": [blosc2.Filter.BITSHUFFLE],
            "nthreads": 1,
        },
        urlpath=str(args.output), contiguous=True,
    )
    offset = 0
    for block, group in zip(blocks, inventory["row_groups"], strict=True):
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
    reopened = blosc2.open(str(args.output))
    if reopened.shape != (rows, features) or reopened.dtype != np.dtype("<f4"):
        raise ValueError("reopened Blosc2 shape or type differs")
    result = {
        "format": "Blosc2 N-D, Zstd, bitshuffle",
        "blosc2_version": blosc2.__version__,
        "source_sha256": inventory["source_sha256"],
        "shape": [rows, features],
        "layout": args.layout,
        "chunks": chunks,
        "blocks": blocks_shape,
        "clevel": args.clevel,
        "raw_bytes": rows * features * 4,
        "stored_bytes_including_metadata": args.output.stat().st_size,
        "stored_sha256": sha256_file(args.output),
        "exact_float32_bits": True,
        "output": str(args.output),
    }
    args.result.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
