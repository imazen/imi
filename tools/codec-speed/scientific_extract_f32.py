#!/usr/bin/env python3
"""Extract Parquet Float32 feature rows as bit-exact, row-major f32le blocks."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import time

import numpy as np
import pyarrow.parquet as pq


def extract_group(source: pq.ParquetFile, index: int, names: list[str], output: Path) -> dict:
    started = time.monotonic_ns()
    table = source.read_row_group(index, columns=names, use_threads=False)
    rows = table.num_rows
    bits = np.empty((rows, len(names)), dtype="<u4")
    for column_index, name in enumerate(names):
        column = table.column(name)
        if column.null_count:
            raise ValueError(f"{name} in row group {index} has {column.null_count} nulls")
        offset = 0
        for chunk in column.chunks:
            if str(chunk.type) != "float":
                raise ValueError(f"{name} is not Float32")
            values = chunk.buffers()[1]
            if values is None:
                raise ValueError(f"{name} has no value buffer")
            raw = np.frombuffer(
                values,
                dtype="<u4",
                count=len(chunk),
                offset=chunk.offset * 4,
            )
            bits[offset : offset + len(chunk), column_index] = raw
            offset += len(chunk)
        if offset != rows:
            raise ValueError(f"{name} has {offset} of {rows} rows")
    raw = memoryview(bits).cast("B")
    digest = hashlib.sha256(raw).hexdigest()
    with output.open("xb") as stream:
        stream.write(raw)
    if output.stat().st_size != rows * len(names) * 4:
        raise ValueError("raw feature block has the wrong length")
    return {
        "row_group": index,
        "rows": rows,
        "features": len(names),
        "bytes": len(raw),
        "sha256": digest,
        "path": str(output),
        "extraction_seconds_diagnostic": (time.monotonic_ns() - started) / 1e9,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--features", type=int, required=True)
    parser.add_argument("--groups", required=True, help="comma-separated row-group indices")
    parser.add_argument("--source-sha256", required=True)
    args = parser.parse_args()
    if sys.byteorder != "little":
        raise ValueError("this raw-bit extractor requires a little-endian host")
    if args.output.exists():
        raise FileExistsError(args.output)
    groups = [int(value) for value in args.groups.split(",")]
    if not groups or len(set(groups)) != len(groups):
        raise ValueError("row-group indices must be nonempty and unique")
    source = pq.ParquetFile(args.source)
    names = [f"f{index}" for index in range(args.features)]
    fields = {field.name: str(field.type) for field in source.schema_arrow}
    if any(fields.get(name) != "float" for name in names):
        raise ValueError("required Float32 feature columns are missing")
    args.output.mkdir(parents=True)
    entries = []
    for index in groups:
        if index < 0 or index >= source.metadata.num_row_groups:
            raise ValueError(f"row group {index} is out of range")
        output = args.output / f"features-rg{index:02}.f32le"
        entry = extract_group(source, index, names, output)
        entries.append(entry)
        print(json.dumps(entry), flush=True)
    manifest = {
        "source": str(args.source),
        "source_sha256_verified_by_inventory": args.source_sha256,
        "semantics": "Each row is all f0..fN Float32 raw bits in schema order; metadata columns are excluded. Parquet nulls in selected features are refused.",
        "blocks": entries,
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
