#!/usr/bin/env python3
"""Fits the per-mode decode cost model the decode-speed tiers use.

fit_cost.py DIR...   (imij-cmp enc/dec output dirs with stats.tsv, run with
                      IMIJ_CMP_NAMES=imij-compact,imij-sh)

run-level:     t = k_rl + p_rl * pixels + a * tokens
split-Huffman: t = k_sh + p_sh * pixels + b * groups + c * coded_block_channels

Least squares on relative error (rows divided by the measured time), one
thread. Prints the constants, the fit error, and held-out checks (fit on
all but one content class / size bucket, test on it).
"""
import csv
import sys
from collections import defaultdict

import numpy as np


def load(dirs):
    rows = []
    for d in dirs:
        sp = defaultdict(dict)
        for r in csv.DictReader(open(f"{d}/speed.tsv"), delimiter="\t"):
            sp[r["file"]][int(r["threads"])] = float(r["best_s"])
        st = {}
        for line in open(f"{d}/stats.tsv"):
            img, s, t, g, b = line.rstrip("\n").split("\t")
            st[(img, s)] = (float(t), float(g), float(b))
        by = {(r["img"], r["codec"], r["setting"]): r for r in csv.DictReader(open(f"{d}/index.tsv"), delimiter="\t")}
        for (img, codec, s), r in by.items():
            if codec != "imij-compact":
                continue
            f = by.get((img, "imij-sh", s))
            if f is None or (img, s) not in st:
                continue
            px = int(r["w"]) * int(r["h"])
            t, g, b = st[(img, s)]
            # Corpus ids: photos are 1xxx-2xxx, scans 6xxx, plots 7xxx, screens 8xxx, clipart 9xxx.
            cls = "photo" if int(img[:4]) < 3000 else "other"
            rows.append(dict(img=img, cls=cls, px=px, tok=t, grp=g, bc=b, q=s,
                             rl={T: sp[r["file"]][T] for T in sp[r["file"]]},
                             sh={T: sp[f["file"]][T] for T in sp[f["file"]]},
                             bytes=int(f["bytes"]) / int(r["bytes"])))
    return rows


def fit(rows, T=1):
    Xr = np.array([[1.0, r["px"], r["tok"]] for r in rows])
    yr = np.array([r["rl"][T] for r in rows])
    Xs = np.array([[1.0, r["px"], r["grp"], r["bc"]] for r in rows])
    ys = np.array([r["sh"][T] for r in rows])
    cr, *_ = np.linalg.lstsq(Xr / yr[:, None], np.ones_like(yr), rcond=None)
    cs, *_ = np.linalg.lstsq(Xs / ys[:, None], np.ones_like(ys), rcond=None)
    return cr, cs


def predict(rows, cr, cs):
    pr = np.array([cr[0] + cr[1] * r["px"] + cr[2] * r["tok"] for r in rows])
    ps = np.array([cs[0] + cs[1] * r["px"] + cs[2] * r["grp"] + cs[3] * r["bc"] for r in rows])
    return pr, ps


def report(name, rows, cr, cs, T=1):
    pr, ps = predict(rows, cr, cs)
    act = np.array([r["sh"][T] / r["rl"][T] for r in rows])
    pred = ps / pr
    err = np.abs(pred - act)
    out = f"{name}: n={len(rows)} ratio err median {np.median(err):.3f} p90 {np.percentile(err, 90):.3f}"
    for th in (0.85, 0.93):
        sel = pred <= th
        slower = int((act[sel] > 1.0).sum())
        gain = act[sel].mean() if sel.any() else float("nan")
        out += f" | switch@{th}: {int(sel.sum())}, slower {slower}, mean ratio {gain:.3f}"
    print(out)


rows = load(sys.argv[1:])
cr, cs = fit(rows)
print("run-level:     k %.1f us, %.3f ns/px, %.3f ns/token" % (cr[0] * 1e6, cr[1] * 1e9, cr[2] * 1e9))
print("split-Huffman: k %.1f us, %.3f ns/px, %.3f ns/group, %.3f ns/coded bc" % (cs[0] * 1e6, cs[1] * 1e9, cs[2] * 1e9, cs[3] * 1e9))
report("all (in-sample)", rows, cr, cs)
report("all, 12 threads (1T model)", rows, cr, cs, T=12)
for c in sorted({r["cls"] for r in rows}):
    tr = [r for r in rows if r["cls"] != c]
    a, b = fit(tr)
    report(f"held-out class {c}", [r for r in rows if r["cls"] == c], a, b)
buckets = [(0, 1e5), (1e5, 2e6), (2e6, 1e8)]
for lo, hi in buckets:
    te = [r for r in rows if lo <= r["px"] < hi]
    if not te:
        continue
    a, b = fit([r for r in rows if not (lo <= r["px"] < hi)])
    report(f"held-out size {lo / 1e6:g}-{hi / 1e6:g} MP", te, a, b)
