#!/usr/bin/env python3
"""Make a reproducible column append from an existing raw pixel matrix.

The added columns repeat the first N columns of each source row. Writes one
source row at a time; files must be new so prior measurement evidence survives.
"""

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("width", type=int)
    parser.add_argument("height", type=int)
    parser.add_argument("bytes_per_pixel", type=int)
    parser.add_argument("added_width", type=int)
    parser.add_argument("added", type=Path)
    parser.add_argument("expected", type=Path)
    args = parser.parse_args()
    if (
        args.width <= 0
        or args.height <= 0
        or args.bytes_per_pixel <= 0
        or not 0 < args.added_width <= args.width
    ):
        parser.error("invalid dimensions or added width")
    row_bytes = args.width * args.bytes_per_pixel
    added_bytes = args.added_width * args.bytes_per_pixel
    if args.source.stat().st_size != row_bytes * args.height:
        parser.error("source byte count does not match dimensions")

    added_hash = hashlib.sha256()
    expected_hash = hashlib.sha256()
    with (
        args.source.open("rb") as source,
        args.added.open("xb") as added,
        args.expected.open("xb") as expected,
    ):
        for _ in range(args.height):
            row = source.read(row_bytes)
            if len(row) != row_bytes:
                raise ValueError("truncated source row")
            right = row[:added_bytes]
            added.write(right)
            expected.write(row)
            expected.write(right)
            added_hash.update(right)
            expected_hash.update(row)
            expected_hash.update(right)
        if source.read(1):
            raise ValueError("source grew after the size check")
    print(
        json.dumps(
            {
                "source": str(args.source),
                "width": args.width,
                "height": args.height,
                "bytes_per_pixel": args.bytes_per_pixel,
                "added_width": args.added_width,
                "added_bytes": added_bytes * args.height,
                "expected_bytes": (row_bytes + added_bytes) * args.height,
                "added_sha256": added_hash.hexdigest(),
                "expected_sha256": expected_hash.hexdigest(),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
