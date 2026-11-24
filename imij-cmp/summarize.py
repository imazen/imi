#!/usr/bin/env python3
"""Tables from imij-cmp output: summarize.py DIR [lossless|lossy].

Lossless: per codec/setting, aggregate bpp (sum bits / sum pixels) and decode
Mpx/s (sum pixels / sum best times) per thread count, overall and per class.

Lossy: per codec, per image, bpp and decode time are interpolated (bpp in
log space, linear in the metric) at fixed SSIMULACRA2 / butteraugli targets;
an image whose curve does not reach a target is left out of that cell (the
count is shown). Speed = sum pixels / sum interpolated times.
"""
import csv
import math
import sys
from collections import defaultdict

d = sys.argv[1]
kind = sys.argv[2] if len(sys.argv) > 2 else "lossless"
rows = list(csv.DictReader(open(f"{d}/index.tsv"), delimiter="\t"))
speed = defaultdict(dict)
for r in csv.DictReader(open(f"{d}/speed.tsv"), delimiter="\t"):
    speed[r["file"]][int(r["threads"])] = float(r["best_s"])
threads = sorted({t for v in speed.values() for t in v})
for r in rows:
    r["px"] = int(r["w"]) * int(r["h"])
    r["bytes"] = int(r["bytes"])
    for k in ("ssim2", "zensim", "ba3", "bamax"):
        r[k] = float(r[k])

# Class from the manifest name is not stored; derive from the corpus layout.
def cls(img):
    for c in ("screen", "clipart", "illust", "doc", "plot", "scan", "photo", "hdr"):
        if c in img:
            return c
    return "photo" if img[:1].isdigit() else "other"


if kind == "lossless":
    groups = defaultdict(list)
    for r in rows:
        groups[(r["codec"], r["setting"])].append(r)
    hdr = "| codec | setting | bpp | " + " | ".join(f"{t}T Mpx/s" for t in threads) + " | " + " | ".join(f"{t}T MB/s out" for t in threads) + " |"
    print(f"Lossless, {len({r['img'] for r in rows})} RGB8 images, aggregate over the set\n")
    print(hdr)
    print("|" + "---|" * (3 + 2 * len(threads)))
    for (c, s), g in groups.items():
        px = sum(r["px"] for r in g)
        bpp = sum(r["bytes"] for r in g) * 8 / px
        mps = [px / sum(speed[r["file"]][t] for r in g) / 1e6 for t in threads]
        print(f"| {c} | {s} | {bpp:.3f} | " + " | ".join(f"{m:.0f}" for m in mps) + " | " + " | ".join(f"{m * 3:.0f}" for m in mps) + " |")
    print("\nPer class: bpp / 1T Mpx/s\n")
    classes = sorted({cls(r["img"]) for r in rows})
    print("| codec | setting | " + " | ".join(classes) + " |")
    print("|" + "---|" * (2 + len(classes)))
    for (c, s), g in groups.items():
        cells = []
        for k in classes:
            gg = [r for r in g if cls(r["img"]) == k]
            px = sum(r["px"] for r in gg)
            bpp = sum(r["bytes"] for r in gg) * 8 / px
            m = px / sum(speed[r["file"]][threads[0]] for r in gg) / 1e6
            cells.append(f"{bpp:.2f} / {m:.0f}")
        print(f"| {c} | {s} | " + " | ".join(cells) + " |")
    sys.exit()


def interp(pts, target, higher_better):
    """pts: (metric, bpp, {t: secs}); returns (bpp, {t: secs}) at target."""
    pts = sorted(pts, key=lambda p: p[0] if higher_better else -p[0])
    for a, b in zip(pts, pts[1:]):
        lo, hi = a[0], b[0]
        if (lo <= target <= hi) if higher_better else (lo >= target >= hi):
            if hi == lo:
                f = 0.0
            else:
                f = (target - lo) / (hi - lo)
            bpp = math.exp(math.log(a[1]) * (1 - f) + math.log(b[1]) * f)
            secs = {t: a[2][t] * (1 - f) + b[2][t] * f for t in a[2]}
            return bpp, secs
    return None


per = defaultdict(lambda: defaultdict(list))
for r in rows:
    per[r["codec"]][r["img"]].append(r)
imgs = sorted({r["img"] for r in rows})
px_of = {r["img"]: r["px"] for r in rows}
codecs = list(per)
for metric, targets, hb, label in [
    ("ssim2", [50, 60, 70, 80, 85, 90], True, "SSIMULACRA2"),
    ("ba3", [4.0, 3.0, 2.0, 1.5, 1.0], False, "butteraugli 3-norm"),
]:
    print(f"\nEqual {label}: bpp (mean over images) and decode Mpx/s; n = images reaching the target\n")
    print("| target | codec | n | bpp | " + " | ".join(f"{t}T Mpx/s" for t in threads) + " |")
    print("|" + "---|" * (4 + len(threads)))
    for tg in targets:
        for c in codecs:
            got = []
            for im in imgs:
                pts = [(r[metric], r["bytes"] * 8 / r["px"], speed[r["file"]]) for r in per[c][im]]
                v = interp(pts, tg, hb)
                if v:
                    got.append((im, v))
            if not got:
                print(f"| {tg} | {c} | 0 | - | " + " | ".join("-" for _ in threads) + " |")
                continue
            bpp = sum(v[0] for _, v in got) / len(got)
            px = sum(px_of[im] for im, _ in got)
            mps = [px / sum(v[1][t] for _, v in got) / 1e6 for t in threads]
            print(f"| {tg} | {c} | {len(got)} | {bpp:.3f} | " + " | ".join(f"{m:.0f}" for m in mps) + " |")
