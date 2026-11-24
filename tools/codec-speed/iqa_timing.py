"""Fixed I01 group: paired worst-of-three native encoding, decode and last-item access.

Inputs loaded before clocks. This is CPU/API timing, not archive/network I/O.
"""
import gzip
import json
import math
import pathlib
import sys
from fast_lossless import execute, metric, require, sha


def run(probe, study, pngs, zenbench, output):
    output.mkdir()
    summary = json.loads((study / "summary.json").read_text())
    require(summary["full_dataset"] and summary["all_pixels_and_item_metadata_exact"], "incomplete storage study")
    group = study / "I01"
    sources = [line.split("\t") for line in (group / "source.tsv").read_text().splitlines()]
    identities = {str(p): sha(p) for p in [probe, zenbench, study / "summary.json", group / "items.tsv"]}
    for row in sources:
        require(sha(pngs / row[0]) == row[2], "PNG changed")
        identities[str(pngs / row[0])] = row[2]
    for p in [*sorted((group / "independent").glob("*.imi")), *sorted(group.glob("*.imi"))]:
        identities[str(p)] = sha(p)
    (output / "identities.json").write_text(json.dumps(identities, indent=2))
    encode_seconds = 0
    rows = []
    pixels = sum(int(row[5]) * int(row[6]) for row in sources)
    for action in ["encode", "decode", "access"]:
        arms = [{"id": f"{arm}_{threads}t", "program": str(probe),
                 "args": ["measure", str(group), str(pngs), arm, action, str(threads)],
                 "timeout_seconds": 120, "revision": identities[str(probe)]}
                for arm in ["independent", "pages", "reference", "previous"]
                for threads in ([1] if action == "encode" else [1, 2, 4, 8])]
        plan = {"schema": 1, "baseline": arms[0], "candidates": arms[1:],
                "cases": [{"id": "I01", "inputs": {}}],
                "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
                           "unit": "ms", "expected": 1, "divide_by": 1},
                "check": {"kind": "json", "prefix": "check=", "pointers": ["/sha256", "/images", "/pixels_exact", "/encoded_bytes_match"]},
                "warmup": "One encode or three decode/access whole-group warmups, one timed operation. Original PNG loading and exact checks outside clock; sequence API reconstruction and returned-raster assertion included in encode. Allocating decode retains outputs. Access decodes only dependency chain to ordinal125, from already loaded full encoded files.",
                "blocks": 3, "aa": True, "deadline_seconds": 180}
        path = output / f"{action}.json"
        path.write_text(json.dumps(plan, indent=2))
        execute([str(zenbench), "pair", str(path), "--wait", "180", "--output", str(output / action)], output / f"{action}.log")
        packet = json.loads(gzip.decompress((output / action / "packet.json.gz").read_bytes()))
        require(packet["complete"] and packet["plan"]["blocks"] == 3, "incomplete timings")
        names = [arm["id"] for arm in arms] + ["__aa"]
        require(len(packet["invocations"]) == len(names) * 3, "observation count")
        values, unique, checks, counts = {}, set(), {}, {}
        for obs in packet["invocations"]:
            require(obs["status"] == "completed" and obs["exit_code"] == 0 and obs["continuation_safe"], "failed process")
            key = (obs["case"], obs["arm"], obs["block"])
            require(key not in unique and key[0] == 0 and 0 <= key[1] < len(names) and 0 <= key[2] < 3, "invalid identity")
            unique.add(key)
            require(len(obs["values_ns"]) == 1 and math.isfinite(obs["values_ns"][0]) and obs["values_ns"][0] > 0, "invalid time")
            values.setdefault(names[key[1]], []).append(obs["values_ns"][0] / 1e6)
            checks.setdefault(key[2], []).append(obs["correctness"])
            for filename, digest in obs["artifacts"].items():
                require(sha(pathlib.Path(filename)) == digest, "artifact changed")
            stdout = (pathlib.Path(obs["output"]) / "stdout.txt").read_text()
            counts.setdefault(names[key[1]], set()).add(metric(stdout, "decoded_images"))
            if action == "encode":
                encode_seconds += metric(stdout, "encode_total_ms") / 1000
        require(all(all(x == group_checks[0] for x in group_checks) for group_checks in checks.values()), "pixel check differs")
        require(all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]), "summary coverage")
        for arm, durations in values.items():
            require(len(durations) == 3 and len(counts[arm]) == 1, "repeat/dependency coverage")
            decoded_images = next(iter(counts[arm]))
            if action != "access":
                require(decoded_images == 126, "whole group coverage")
            else:
                native = arm.rsplit("_", 1)[0] if arm != "__aa" else "independent"
                expected = 1 if native == "independent" else int((group / f"{native}-access.tsv").read_text().splitlines()[-1].split("\t")[1])
                require(decoded_images == expected, "access dependencies differ")
            work_pixels = pixels if action != "access" else int(sources[-1][5]) * int(sources[-1][6])
            rows.append({"action": action, "arm": arm, "milliseconds": durations,
                         "worst_ms": max(durations), "mpx_s": work_pixels / (1000 * max(durations)),
                         "decoded_images": decoded_images})
    require(all(sha(pathlib.Path(p)) == digest for p, digest in identities.items()), "identity changed")
    result = {"group": "I01", "images": 126, "pixels": pixels, "all_encode_seconds": encode_seconds,
              "scope": "native CPU/API, loaded encoded inputs; archive/network I/O not measured", "rows": rows}
    (output / "summary.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 6, "iqa_timing.py PROBE FULL-STUDY ORIGINAL-PNG-DIR ZENBENCH NEW-OUTPUT")
    run(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
