#!/usr/bin/env python3
"""Read one verified F32 row group with the existing scientific stores.

This probes feature decoding, not the full zensim trainer: targets, reference
identity, Float64 widening and optimizer work are deliberately excluded.
"""

import argparse
import hashlib
import importlib
import json
import statistics
import time
from pathlib import Path

import numpy as np


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def store_digest(path: Path) -> tuple[str, int]:
    if path.is_file():
        return sha256_file(path), path.stat().st_size
    digest = hashlib.sha256()
    total = 0
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        size = item.stat().st_size
        digest.update(
            f"{item.relative_to(path)}\t{size}\t{sha256_file(item)}\n".encode()
        )
        total += size
    return digest.hexdigest(), total


def import_backend(kind: str) -> None:
    """Make Python module loading visible and separate from store opening."""
    module = {
        "parquet": "pyarrow.parquet",
        "zarr": "zarr",
        "blosc2": "blosc2",
        "safetensors-zstd": "safetensors.numpy",
    }[kind]
    importlib.import_module(module)
    if kind == "safetensors-zstd":
        importlib.import_module("zstandard")


def reader(
    kind: str,
    path: Path,
    features: int,
    row_group: int,
    raw_bytes: int,
    region: tuple[int, int, int, int] | None,
):
    """Return the read callable after opening the format's metadata handle."""
    x, y, width, height = region or (0, 0, features, None)
    if kind == "parquet":
        import pyarrow.parquet as pq

        parquet = pq.ParquetFile(path)
        names = [f"f{i}" for i in range(x, x + width)]
        if row_group >= parquet.num_row_groups:
            raise ValueError("row group is absent")

        def read():
            table = parquet.read_row_group(row_group, columns=names, use_threads=False)
            if table.column_names != names:
                raise ValueError("Parquet feature column order differs")
            return np.column_stack(
                [table.column(name).slice(y, height).to_numpy() for name in names]
            )

    elif kind == "zarr":
        import zarr

        array = zarr.open_array(store=str(path), mode="r")

        def read():
            return np.asarray(
                array[:] if region is None else array[y : y + height, x : x + width]
            )

    elif kind == "blosc2":
        import blosc2

        array = blosc2.open(str(path))

        def read():
            return np.asarray(
                array[:] if region is None else array[y : y + height, x : x + width]
            )

    elif kind == "safetensors-zstd":
        import safetensors.numpy
        import zstandard

        decoder = zstandard.ZstdDecompressor()

        def read():
            compressed = path.read_bytes()
            decoded = decoder.decompress(
                compressed, max_output_size=raw_bytes + (1 << 20)
            )
            matrix = safetensors.numpy.load(decoded)["features"]
            return matrix if region is None else matrix[y : y + height, x : x + width]

    else:
        raise ValueError(f"unsupported format {kind}")
    return read


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "kind", choices=("parquet", "zarr", "blosc2", "safetensors-zstd")
    )
    parser.add_argument("raw", type=Path)
    parser.add_argument("store", type=Path)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--features", type=int, required=True)
    parser.add_argument("--row-group", type=int, default=0)
    parser.add_argument(
        "--region", nargs=4, type=int, metavar=("X", "Y", "WIDTH", "HEIGHT")
    )
    parser.add_argument("--hot-runs", type=int, default=11)
    parser.add_argument("--raw-sha256", required=True)
    parser.add_argument("--store-sha256", required=True)
    args = parser.parse_args()
    if args.rows <= 0 or args.features <= 0 or args.hot_runs <= 0 or args.row_group < 0:
        raise ValueError("invalid matrix shape, group or run count")
    region = tuple(args.region) if args.region else None
    if region is not None:
        x, y, width, height = region
        if (
            x < 0
            or y < 0
            or width <= 0
            or height <= 0
            or x + width > args.features
            or y + height > args.rows
        ):
            raise ValueError("region is outside the raw matrix")
    raw_bytes = args.rows * args.features * 4
    if args.raw.stat().st_size != raw_bytes or sha256_file(args.raw) != args.raw_sha256:
        raise ValueError("raw Float32 source identity differs")
    before_sha, store_bytes = store_digest(args.store)
    if before_sha != args.store_sha256:
        raise ValueError("stored representation identity differs")
    reference = np.memmap(
        args.raw, dtype="<u4", mode="r", shape=(args.rows, args.features)
    )
    if region is not None:
        reference = reference[y : y + height, x : x + width]

    started = time.perf_counter_ns()
    import_backend(args.kind)
    backend_import_ns = time.perf_counter_ns() - started
    started = time.perf_counter_ns()
    read = reader(
        args.kind, args.store, args.features, args.row_group, raw_bytes, region
    )
    open_ns = time.perf_counter_ns() - started

    def checked_read() -> tuple[int, np.ndarray]:
        started = time.perf_counter_ns()
        actual = read()
        elapsed = time.perf_counter_ns() - started
        if actual.shape != reference.shape or actual.dtype != np.dtype("<f4"):
            raise ValueError("decoded shape or Float32 type differs")
        if not np.array_equal(actual.view("<u4"), reference):
            raise ValueError("decoded Float32 bits differ from the Parquet source")
        return elapsed, actual

    first_ns, actual = checked_read()
    del actual
    hot_ns = []
    for _ in range(args.hot_runs):
        elapsed, actual = checked_read()
        hot_ns.append(elapsed)
        del actual
    after_sha, after_bytes = store_digest(args.store)
    if (after_sha, after_bytes) != (before_sha, store_bytes):
        raise ValueError("stored representation changed during the probe")
    print(
        json.dumps(
            {
                "kind": args.kind,
                "store": str(args.store),
                "raw": str(args.raw),
                "rows": args.rows,
                "features": args.features,
                "row_group": args.row_group,
                "region": region,
                "store_bytes": store_bytes,
                "store_sha256": before_sha,
                "raw_sha256": args.raw_sha256,
                "backend_import_ns": backend_import_ns,
                "open_ns": open_ns,
                "first_decode_ns": first_ns,
                "hot_median_ns": statistics.median(hot_ns),
                "hot_worst_ns": max(hot_ns),
                "hot_runs": args.hot_runs,
                "exact_float32_bits": True,
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
