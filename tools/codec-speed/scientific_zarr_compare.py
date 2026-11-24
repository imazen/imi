#!/usr/bin/env python3
"""Encode one bit-exact Float32 feature block as Zarr/Blosc or Blosc2 N-D."""

import argparse
import hashlib
import json
from pathlib import Path
import sys

import blosc2
import numpy as np
import zarr


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def inventory(path: Path) -> tuple[int, int, str]:
    files = [item for item in path.rglob("*") if item.is_file()] if path.is_dir() else [path]
    digest = hashlib.sha256()
    total = 0
    for item in sorted(files):
        relative = item.relative_to(path) if path.is_dir() else Path(item.name)
        size = item.stat().st_size
        file_sha256 = sha256_file(item)
        total += size
        digest.update(f"{relative}\t{size}\t{file_sha256}\n".encode())
    return total, len(files), digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("result", type=Path)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--features", type=int, required=True)
    parser.add_argument("--format", choices=("zarr-blosc", "blosc2-nd"), required=True)
    parser.add_argument("--layout", choices=("whole", "rows", "grid"), required=True)
    parser.add_argument("--clevel", type=int, default=5)
    args = parser.parse_args()
    if sys.byteorder != "little":
        raise ValueError("raw Float32 bits require a little-endian host")
    if not 0 <= args.clevel <= 9 or args.rows <= 0 or args.features <= 0:
        raise ValueError("invalid compression level or shape")
    if args.output.exists() or args.result.exists():
        raise FileExistsError("output or result already exists")
    expected = args.rows * args.features * 4
    if args.raw.stat().st_size != expected:
        raise ValueError("raw Float32 byte length differs from the declared shape")
    shape = (args.rows, args.features)
    chunks = {
        "whole": shape,
        "rows": (min(1024, args.rows), args.features),
        "grid": (min(1024, args.rows), min(64, args.features)),
    }[args.layout]
    raw_sha256 = sha256_file(args.raw)
    source = np.memmap(args.raw, dtype="<f4", mode="r", shape=shape)
    if args.format == "zarr-blosc":
        array = zarr.create_array(
            store=str(args.output),
            shape=shape,
            dtype="<f4",
            chunks=chunks,
            compressors=[zarr.codecs.BloscCodec(cname="zstd", clevel=args.clevel, shuffle="bitshuffle")],
            zarr_format=3,
        )
        array[:] = source
        decoded = np.asarray(array[:])
        codec = "Zarr v3 built-in Blosc (Blosc1 wire), Zstd, bitshuffle"
    else:
        blocks = (min(128, chunks[0]), min(64, chunks[1]))
        array = blosc2.asarray(
            source,
            chunks=chunks,
            blocks=blocks,
            cparams={
                "codec": blosc2.Codec.ZSTD,
                "clevel": args.clevel,
                "filters": [blosc2.Filter.BITSHUFFLE],
                "nthreads": 1,
            },
            urlpath=str(args.output),
            contiguous=True,
        )
        decoded = np.asarray(blosc2.open(str(args.output)))
        codec = "Blosc2 N-D, Zstd, bitshuffle"
    if decoded.dtype != np.dtype("<f4") or decoded.shape != shape:
        raise ValueError("decoded dtype or shape differs")
    if not np.array_equal(decoded.view("<u4"), source.view("<u4")):
        raise ValueError("decoded Float32 bit patterns differ")
    stored_bytes, file_count, content_sha256 = inventory(args.output)
    result = {
        "format": args.format,
        "codec": codec,
        "zarr_version": zarr.__version__,
        "blosc2_version": blosc2.__version__,
        "numpy_version": np.__version__,
        "layout": args.layout,
        "shape": shape,
        "chunks": chunks,
        "clevel": args.clevel,
        "raw_bytes": expected,
        "raw_sha256": raw_sha256,
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
