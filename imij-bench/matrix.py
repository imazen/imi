#!/usr/bin/env python3
"""Prepare persistent cases, run zenbench comparisons, or isolate heap/RSS probes."""
import argparse
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
from datetime import datetime, timezone

REPO = Path(__file__).resolve().parent.parent


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def capture(*args, cwd=REPO):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def provenance(binary):
    dirty = capture("git", "status", "--porcelain")
    # A commit alone does not identify a working-copy binary.
    sources = {}
    for root in (REPO / "imij/src", REPO / "imij-bench/benches"):
        sources.update({str(p.relative_to(REPO)): digest(p) for p in sorted(root.rglob("*.rs"))})
    siblings = {}
    for name in ("zenbench", "zenpng", "zenbitmaps", "zenmetrics", "zenjpeg"):
        root = REPO.parent / name
        changed = capture("git", "ls-files", "-z", "-m", "-o", "--exclude-standard", cwd=root)
        siblings[name] = {
            "git": capture("git", "rev-parse", "HEAD", cwd=root),
            "dirty": capture("git", "status", "--porcelain", cwd=root),
            "working_files_sha256": {
                p: digest(root / p) for p in sorted(set(changed.split("\0")))
                if p and (root / p).is_file()
            },
        }
    return {
        "utc": datetime.now(timezone.utc).isoformat(),
        "git": capture("git", "rev-parse", "HEAD"), "dirty": dirty,
        "sources_sha256": sources, "binary": str(binary), "binary_sha256": digest(binary),
        "build_info": json.loads(capture(binary, "build-info")),
        "rustc": capture("rustc", "--version", "--verbose"),
        "platform": platform.platform(), "machine": platform.machine(),
        "hostname": platform.node(),
        "environment": {k: v for k, v in os.environ.items()
                        if k.startswith(("IMIJ_", "ZENBENCH_")) or k in ("RUSTFLAGS", "RAYON_NUM_THREADS")},
        "load_average": os.getloadavg() if hasattr(os, "getloadavg") else None,
        "cargo_lock_sha256": digest(REPO / "imij-bench/Cargo.lock"),
        "siblings": {name: state["git"] for name, state in siblings.items()},
        "sibling_worktrees": siblings,
        "source_hash_scope": "working files observed at run start; preserve a build snapshot to bind them to the binary",
        "memory_accounting": "process heap/RSS includes setup; no per-thread peak summation",
    }


def binary_arg(args):
    p = Path(args.binary).resolve()
    if not p.is_file():
        raise ValueError(f"benchmark executable missing: {p}")
    return p


def make_spec(args):
    profiles = []
    modes = args.modes.split(",")
    for mode in modes:
        colors = ["xyb"] if mode == "lossless" else args.colors.split(",")
        qualities = [90] if mode == "lossless" else [float(q) for q in args.qualities.split(",")]
        for color, quality, threads, rows, groups, effort in itertools.product(
                colors, qualities, args.encode_threads.split(","),
                args.stripe_rows.split(","), args.group_cols.split(","), args.efforts.split(",")):
            name = f"{mode}-{color}-q{quality:g}-t{threads}-s{rows}-g{groups}-effort{effort}"
            profiles.append({"name": name, "mode": mode, "color": color, "quality": quality,
                             "effort": float(effort),
                             "encode_threads": int(threads),
                             "stripe_rows": None if rows == "auto" else int(rows),
                             "group_cols": None if groups == "auto" else int(groups)})
    value = {"corpus": str(Path(args.corpus).resolve()), "classes": args.classes,
             "variants": args.variants, "profiles": profiles,
             "operations": args.operations.split(","),
             "decode_threads": [int(t) for t in args.decode_threads.split(",")],
             "pool_threads": args.pool_threads}
    print(json.dumps(value, indent=2))


def prepare(args):
    binary = binary_arg(args)
    out = Path(args.output).resolve()
    if out.exists():
        raise ValueError("use a new output directory")
    spec = Path(args.spec).resolve()
    prov = provenance(binary)
    prov["spec_sha256"] = digest(spec)
    corpus = Path(json.loads(spec.read_text())["corpus"])
    prov["corpus_manifest_sha256"] = digest(corpus)
    try:
        subprocess.run([binary, "prepare", spec, out], check=True)
    finally:
        if out.exists():
            write_json(out / "provenance.json", prov)
            shutil.copyfile(spec, out / "spec.json")
            shutil.copyfile(corpus, out / "corpus.tsv")
    # Deduplicate and address saved source pixels and bitstreams by content.
    records = json.loads((out / "index.json").read_text())
    artifacts = out / "artifacts"
    artifacts.mkdir()
    relocated = {}
    hashes = {}
    for record in records:
        for field in ("encoded", "pixels"):
            old = Path(record[field])
            if old not in relocated:
                sha = digest(old)
                dest = artifacts / (sha + old.suffix)
                if dest.exists():
                    old.unlink()
                else:
                    old.rename(dest)
                relocated[old] = str(dest)
                hashes[str(dest)] = sha
            record[field] = relocated[old]
        write_json(out / "cases" / f"{record['id']}.json", record)
    write_json(out / "index.json", records)
    write_json(out / "artifacts.json", hashes)
    print(f"{len(records)} verified cases; artifacts and provenance: {out}")


def timing_status(report, requested_rounds):
    """Check achieved rounds; zenbench's unreliable flag alone is insufficient."""
    issues = []
    if report.get("unreliable", True):
        issues.append("zenbench marked the run unreliable or omitted that flag")
    comparisons = report.get("comparisons", [])
    if not comparisons:
        issues.append("no comparison results")
    for comparison in comparisons:
        name = comparison.get("group_name", "unnamed group")
        rounds = comparison.get("completed_rounds", 0)
        if rounds < requested_rounds:
            issues.append(f"{name}: completed {rounds}/{requested_rounds} rounds")
        for analysis in comparison.get("analyses", []):
            samples = analysis[2].get("n_samples", 0)
            if samples < requested_rounds:
                issues.append(f"{name}: only {samples} paired samples")
    return {"requested_rounds": requested_rounds, "passed": not issues, "issues": issues}


def time_cases(args):
    if args.rounds < 2:
        raise ValueError("at least two rounds required")
    binary = binary_arg(args)
    run = Path(args.run).resolve()
    records = json.loads((run / "index.json").read_text())
    groups = sorted({r["comparison"] for r in records
                     if args.operation is None or r["operation"] == args.operation})
    if not groups:
        raise ValueError("no comparisons matched")
    out = Path(args.output).resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, ZENBENCH_NO_SAVE="1")
    prov = provenance(binary)
    prov["environment"]["ZENBENCH_NO_SAVE"] = "1"
    write_json(out / "provenance.json", prov)
    results = []
    for i, group in enumerate(groups):
        dest = out / f"{i}.json"
        # Fresh process per image/operation limits resident comparison fixtures
        # and isolates process-global kernel-selection state.
        with (out / f"{i}.log").open("w") as log:
            subprocess.run([binary, "time", run / "index.json", dest, str(args.rounds), group],
                           stdout=log, stderr=subprocess.STDOUT, check=True, env=env)
        status = timing_status(json.loads(dest.read_text()), args.rounds)
        results.append({"comparison": group, "result": str(dest), "validation": status})
        write_json(out / "index.json", results)
        print(f"{group}: {'complete' if status['passed'] else 'incomplete/unreliable'}", flush=True)
    if any(not result["validation"]["passed"] for result in results):
        raise ValueError(f"incomplete or unreliable timings; raw results and reasons retained in {out}")


def memory(args):
    binary = binary_arg(args)
    case = Path(args.run).resolve() / "cases" / f"{args.case}.json"
    record = json.loads(case.read_text())
    out = Path(args.output).resolve()
    out.mkdir(parents=True, exist_ok=False)
    write_json(out / "provenance.json", provenance(binary))
    write_json(out / "case.json", record)
    command = [str(binary), "probe", str(case), str(args.repetitions)]
    # RSS and heap use separate fresh processes. Timing from these runs is
    # diagnostic only and must not be merged into zenbench timing samples.
    with (out / "rss.log").open("w") as log:
        subprocess.run(["/usr/bin/time", "-v", "-o", str(out / "rss.txt"), *command],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    with (out / "heaptrack.log").open("w") as log:
        subprocess.run(["heaptrack", "--record-only", "-o", str(out / "heap"), *command],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    traces = list(out.glob("heap*.gz")) + list(out.glob("heap*.zst"))
    if len(traces) != 1:
        raise ValueError(f"expected one heaptrack trace, got {traces}")
    with (out / "heap.txt").open("w") as report:
        subprocess.run(["heaptrack_print", "-f", str(traces[0])],
                       stdout=report, stderr=subprocess.STDOUT, check=True)
    print(out)


def score(args):
    binary = binary_arg(args)
    run, out = Path(args.run).resolve(), Path(args.output).resolve()
    if out.exists():
        raise ValueError("use a new score directory")
    prov = provenance(binary)
    try:
        subprocess.run([binary, "score", run / "index.json", out], check=True)
    finally:
        if out.exists():
            write_json(out / "provenance.json", prov)


def rd_frontier(points):
    """Increasing quality/log-rate Pareto frontier; reject nonfinite input."""
    if any(not math.isfinite(q) or not math.isfinite(b) or b <= 0 for q, b in points):
        raise ValueError("invalid rate-distortion point")
    frontier = []
    for q, size in sorted(set(points), key=lambda p: (p[1], -p[0])):
        if not frontier or q > frontier[-1][0]:
            frontier.append((q, math.log(size)))
    return frontier


def bd_rate(base, candidate):
    """Integrate piecewise-linear log(rate) over the common quality range."""
    a, b = rd_frontier(base), rd_frontier(candidate)
    if len(a) < 2 or len(b) < 2:
        raise ValueError("BD-rate needs at least two frontier points")
    lo, hi = max(a[0][0], b[0][0]), min(a[-1][0], b[-1][0])
    if lo >= hi:
        raise ValueError("BD-rate curves have no common quality interval")

    def integral(curve):
        total = 0.0
        for (x0, y0), (x1, y1) in zip(curve, curve[1:]):
            left, right = max(x0, lo), min(x1, hi)
            if left < right:
                slope = (y1 - y0) / (x1 - x0)
                yl, yr = y0 + slope * (left - x0), y0 + slope * (right - x0)
                total += (yl + yr) * 0.5 * (right - left)
        return total

    return {"percent": math.expm1((integral(b) - integral(a)) / (hi - lo)) * 100,
            "quality_range": [lo, hi], "frontier_points": [len(a), len(b)],
            "sampled_quality_ceilings": [a[-1][0], b[-1][0]],
            "input_points": [len(base), len(candidate)]}


def compare_rd(args):
    records = json.loads((Path(args.scores) / "index.json").read_text())
    groups = {}
    metrics = {"ssimulacra2": 1, "butteraugli_p3": -1, "butteraugli_max": -1,
               "cvvdp_fhd_jod": 1, "vmaf_v1_fhd": 1}
    for record in records:
        case = record["case"]
        profile = dict(case["profile"])
        variant = profile.pop(args.axis)
        profile.pop("quality")
        profile.pop("name")
        key = (str(case["pixels"]), json.dumps(profile, sort_keys=True))
        stats = json.loads(Path(record["stats"]).read_text())
        settings = stats.get("metric_settings")
        entry = groups.setdefault(key, {"image": case["comparison"], "profile": profile,
                                        "metric_settings": settings, "curves": {}})
        if entry["metric_settings"] != settings:
            raise ValueError(f"metric settings differ within {case['comparison']}; re-score consistently")
        curve = entry["curves"].setdefault(variant, {})
        for metric, sign in metrics.items():
            points = curve.setdefault(metric, set())
            value = stats.get(metric)
            if value is not None:
                points.add((value * sign, case["encoded_bytes"]))
    results = []
    baseline = json.loads(args.baseline)
    for entry in groups.values():
        curves = entry.pop("curves")
        if baseline not in curves:
            raise ValueError(f"missing baseline in {entry['image']}")
        for variant, candidate in curves.items():
            if variant == baseline:
                continue
            result = dict(entry, axis=args.axis, baseline=baseline, candidate=variant, metrics={})
            for metric in metrics:
                try:
                    result["metrics"][metric] = bd_rate(list(curves[baseline][metric]), list(candidate[metric]))
                except ValueError as error:
                    result["metrics"][metric] = {"unavailable": str(error)}
            results.append(result)
    output = Path(args.output)
    if output.exists():
        raise ValueError("BD-rate output already exists")
    write_json(output, {"method": "piecewise-linear log-rate integration; Pareto frontier; common quality only; no extrapolation", "comparisons": results})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    spec = sub.add_parser("spec", help="emit an explicit permutation specification")
    spec.add_argument("corpus")
    spec.add_argument("--classes")
    spec.add_argument("--variants", action="store_true")
    spec.add_argument("--modes", default="runlevel,split,huffman")
    spec.add_argument("--colors", default="xyb,ycocg")
    spec.add_argument("--qualities", default="30,60,90")
    spec.add_argument("--encode-threads", default="1")
    spec.add_argument("--efforts", default="10", help="comma-separated encoder efforts, 0..10 including fractions")
    spec.add_argument("--decode-threads", default="1,4,12")
    spec.add_argument("--stripe-rows", default="auto")
    spec.add_argument("--group-cols", default="auto")
    spec.add_argument("--pool-threads", type=int, default=12)
    spec.add_argument("--operations", default="encode,into,alloc,stream,reader,reader-file")
    spec.set_defaults(func=make_spec)
    prep = sub.add_parser("prepare")
    prep.add_argument("--binary", required=True)
    prep.add_argument("spec")
    prep.add_argument("output")
    prep.set_defaults(func=prepare)
    timing = sub.add_parser("time")
    timing.add_argument("--binary", required=True)
    timing.add_argument("run")
    timing.add_argument("output")
    timing.add_argument("--operation")
    timing.add_argument("--rounds", type=int, default=20)
    timing.set_defaults(func=time_cases)
    mem = sub.add_parser("memory")
    mem.add_argument("--binary", required=True)
    mem.add_argument("run")
    mem.add_argument("case", type=int)
    mem.add_argument("output")
    mem.add_argument("--repetitions", type=int, default=1)
    mem.set_defaults(func=memory)
    scoring = sub.add_parser("score")
    scoring.add_argument("--binary", required=True)
    scoring.add_argument("run")
    scoring.add_argument("output")
    scoring.set_defaults(func=score)
    rd = sub.add_parser("bd-rate")
    rd.add_argument("scores")
    rd.add_argument("output")
    rd.add_argument("--axis", choices=["fdct", "idct_fma", "effort", "mode", "color"], default="fdct")
    rd.add_argument("--baseline", default="1", help="JSON value: e.g. 1 or false")
    rd.set_defaults(func=compare_rd)
    args = parser.parse_args()
    try:
        args.func(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"matrix: {error}\n")


if __name__ == "__main__":
    main()
