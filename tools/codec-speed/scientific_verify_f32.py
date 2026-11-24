#!/usr/bin/env python3
"""Independently compare a row-major f32le block with its Parquet row group."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("block", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--row-group", type=int, required=True)
    parser.add_argument("--features", type=int, required=True)
    parser.add_argument("--expected-block-sha256", required=True)
    args = parser.parse_args()

    pf = pq.ParquetFile(args.source)
    rows = pf.metadata.row_group(args.row_group).num_rows
    if args.block.stat().st_size != rows * args.features * 4:
        raise ValueError("block length does not match the row group")
    digest = hashlib.sha256()
    with args.block.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    if digest.hexdigest() != args.expected_block_sha256:
        raise ValueError("block hash changed")

    matrix = np.memmap(args.block, dtype="<u4", mode="r", shape=(rows, args.features))
    names = [f"f{index}" for index in range(args.features)]
    table = pf.read_row_group(args.row_group, columns=names, use_threads=False)
    checked = 0
    for index, name in enumerate(names):
        column = table.column(name)
        if column.null_count:
            raise ValueError(f"{name} has null values")
        position = 0
        for chunk in column.chunks:
            raw = np.frombuffer(
                chunk.buffers()[1], dtype="<u4", count=len(chunk), offset=chunk.offset * 4
            )
            if not np.array_equal(raw, matrix[position : position + len(chunk), index]):
                raise ValueError(f"{name} differs in row group {args.row_group}")
            position += len(chunk)
        if position != rows:
            raise ValueError(f"{name} has a row-count mismatch")
        checked += rows
    result = {
        "source": str(args.source),
        "block": str(args.block),
        "block_sha256": digest.hexdigest(),
        "row_group": args.row_group,
        "rows": rows,
        "features": args.features,
        "checked_float32_bit_patterns": checked,
        "mismatches": 0,
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
