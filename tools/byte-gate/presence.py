#!/usr/bin/env python3
"""Presence-bit write frequencies (format version 2, design decision 1).

    presence.py [--grid PRESENCE.tsv] [--files DIR ...]

--grid: the output of presence.rs (key columns, header present, feature
mask, lossy present or "-"). --files: directories of version 2 files
(`*.imi`, e.g. the decode-parity fixtures), whose top-level header and, for
lossy files, lossy head are read here. Prints, per source, how often each
header field, feature and lossy parameter is written.
"""
import argparse
from pathlib import Path

HEADER = ["coding", "sample", "features", "channels", "orientation", "decode_speed", "cicp", "sig_bits",
          "alpha_mode"]
FEATURES = ["seed_bank_1", "exact_blocks", "signed_samples", "wide_samples", "near_lossless", "float_shift",
            "mixed_pages", "dequant_presets", "explicit_tables", "recon_bias", "color_transform", "offset_map"]
LOSSY = ["seed", "entropy", "matrix", "dc", "records", "full_states", "floor", "scale", "mul"]


def varint(b, at):
    v = 0
    for i in range(10):
        x = b[at]
        at += 1
        v |= (x & 127) << (7 * i)
        if x < 128:
            break
    return v, at


def file_presence(f):
    at = 8
    hl, at = varint(f, at)
    h = f[at:at + hl]
    end = at + hl
    if h[0] != 2:
        raise ValueError("not header version 2")
    present, i = varint(h, 1)
    features = 0
    if present & 4:
        features, i = varint(h, i)
    lossy = None
    if present & 1 and h[i] != 0:
        c = end
        while True:
            tag = f[c:c + 4]
            n, c = varint(f, c + 4)
            if tag == b"DATA":
                _, d = varint(f, c + 1)
                lossy, _ = varint(f, d)
                break
            c += n - 1
    return present, features, lossy


def table(title, rows):
    n = len(rows)
    nl = sum(1 for r in rows if r[2] is not None)
    print(f"\n## {title}: {n} files ({nl} lossy)\n")
    print("| field | bit | written | % |")
    print("|---|---:|---:|---:|")
    for b, name in enumerate(HEADER):
        k = sum(1 for r in rows if r[0] >> b & 1)
        print(f"| header {name} | {b} | {k} | {100 * k / n:.1f} |")
    for b, name in enumerate(FEATURES):
        k = sum(1 for r in rows if r[1] >> b & 1)
        if k:
            print(f"| feature {name} | {b} | {k} | {100 * k / n:.1f} |")
    for b, name in enumerate(LOSSY):
        k = sum(1 for r in rows if r[2] is not None and r[2] >> b & 1)
        if nl:
            print(f"| lossy {name} | {b} | {k} | {100 * k / nl:.1f} of lossy |")
    over = sum(1 for r in rows if r[0] >= 128)
    print(f"\nHeaders whose `present` needs two bytes: {over} of {n}.")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--grid", type=Path)
    ap.add_argument("--files", type=Path, nargs="*", default=[])
    a = ap.parse_args()
    if a.grid:
        rows = []
        for line in open(a.grid):
            f = line.rstrip("\n").split("\t")
            rows.append((int(f[-3]), int(f[-2]), None if f[-1] == "-" else int(f[-1])))
        table(f"byte-gate grid ({a.grid.name})", rows)
    for d in a.files:
        rows = [file_presence(p.read_bytes()) for p in sorted(d.glob("*.imi"))]
        table(f"files in {d}", rows)


if __name__ == "__main__":
    main()
