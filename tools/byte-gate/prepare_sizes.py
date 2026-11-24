#!/usr/bin/env python3
"""Inputs of the bytes report (sizes.rs): raw RGB8 files named
`<name>_<w>_<h>.rgb8` in OUT.

* 64 px: every byte-gate image (1024 px crops, README.md) downscaled to
  64 x 64 with Lanczos (never upscaled).
* 4096 px: the centre 4096 x 4096 crop of the first COUNT photos, in name
  order, of SOURCES (JPEG files at least 4096 px on both sides).

    prepare_sizes.py GATE_RAW SOURCES OUT [--count 6]

Writes OUT/SHA256SUMS. PIL decodes and resamples (inputs only; nothing it
writes ships).
"""
import argparse
import hashlib
from pathlib import Path

from PIL import Image


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("gate_raw", type=Path)
    ap.add_argument("sources", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--count", type=int, default=6)
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    written = []
    for p in sorted(a.gate_raw.glob("*.rgb8")):
        name, w, h = p.stem.rsplit("_", 2)
        w, h = int(w), int(h)
        im = Image.frombytes("RGB", (w, h), p.read_bytes())
        side = min(w, h)
        im = im.crop(((w - side) // 2, (h - side) // 2, (w - side) // 2 + side, (h - side) // 2 + side))
        small = im.resize((64, 64), Image.Resampling.LANCZOS)
        q = a.out / f"{name}-64_64_64.rgb8"
        q.write_bytes(small.tobytes())
        written.append(q)
    n = 0
    for p in sorted(a.sources.glob("*.jpg")):
        if n == a.count:
            break
        with Image.open(p) as im:
            w, h = im.size
            if min(w, h) < 4096:
                continue
            im = im.convert("RGB")
            x, y = (w - 4096) // 2, (h - 4096) // 2
            big = im.crop((x, y, x + 4096, y + 4096))
        q = a.out / f"{p.stem}-4096_4096_4096.rgb8"
        q.write_bytes(big.tobytes())
        written.append(q)
        n += 1
    with open(a.out / "SHA256SUMS", "w") as s:
        for q in sorted(written):
            s.write(f"{hashlib.sha256(q.read_bytes()).hexdigest()}  {q.name}\n")
    print(f"{len(written)} images in {a.out}")


if __name__ == "__main__":
    main()
