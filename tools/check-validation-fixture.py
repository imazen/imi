#!/usr/bin/env python3
"""Independently check the raster produced from validation_fixture's GrayU8 image."""
import argparse
import hashlib
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("raster", type=Path)
parser.add_argument("width", type=int)
parser.add_argument("height", type=int)
args = parser.parse_args()
if args.width <= 0 or args.height <= 0:
    parser.error("dimensions must be positive")
digest = hashlib.sha256()
with args.raster.open("rb") as raster:
    for y in range(args.height):
        expected = b"".join(
            bytes([(y // 17 + x // 23) % 256]) * min(1024, args.width - x)
            for x in range(0, args.width, 1024)
        )
        actual = raster.read(args.width)
        if actual != expected:
            raise SystemExit(f"pixel mismatch or truncation at row {y}")
        digest.update(actual)
    if raster.read(1):
        raise SystemExit("unexpected bytes after raster")
print(f"verified_pixels={args.width * args.height} sha256={digest.hexdigest()}")
