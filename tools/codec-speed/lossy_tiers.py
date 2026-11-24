"""Bounded explicit lossy-tier comparison; no model fitting or policy change."""
import gzip
import json
import math
import pathlib
import shutil
import sys
from fast_lossless import execute, metric, require, sha

IMAGES = ["1000", "1400", "1600"]
QUALITIES = [20, 50, 80]
POLICIES = ["compact", "balanced", "fast", "huffman", "default"]
PIXELS = 1024 * 1024


def run(probe, inspector, raw, zenbench, root):
    root.mkdir()
    files = root / "files"
    files.mkdir()
    identities = {str(p): sha(p) for p in [probe, inspector, zenbench]}
    preparations, encode_ms = {}, 0.0
    for image in IMAGES:
        source = raw / f"{image}.rgb"
        require(source.stat().st_size == PIXELS * 3, "wrong RGB crop size")
        identities[str(source)] = sha(source)
        shutil.copyfile(source, files / source.name)
        for quality in QUALITIES:
            for policy in POLICIES:
                key = f"{image}-q{quality}-{policy}"
                output = files / f"{key}.imi"
                log = execute([str(probe), "prepare", "imij-" + policy, str(quality),
                               "1", str(files / source.name), str(output)], root / f"prepare-{key}.log")
                encode_ms += metric(log, "encode_total_ms")
                require(encode_ms < 60000, "additional encode budget exceeded")
                inspection = execute([str(inspector), str(output)], root / f"inspect-{key}.log")
                mode = int(metric(inspection, "entropy"))
                require(mode in [0, 3], "unexpected entropy")
                if policy in ["default", "huffman"]:
                    require(mode == (3 if policy == "huffman" else 0), "forced coder mismatch")
                preparations[key] = {"entropy": mode, "bytes": output.stat().st_size,
                                     "encoded_sha256": sha(output)}
    (root / "identities.json").write_text(json.dumps(identities, indent=2))
    (root / "preparations.json").write_text(json.dumps(preparations, indent=2))
    preparation_ms = encode_ms
    all_cells, aa, counts = {}, {}, {}
    for action in ["encode", "decode"]:
        cases = []
        for image in IMAGES:
            for quality in QUALITIES:
                for policy in ([None] if action == "encode" else POLICIES):
                    inputs = {"raw": str(files / f"{image}.rgb")}
                    fields = {"quality": str(quality)}
                    if policy is None:
                        inputs.update({p: str(files / f"{image}-q{quality}-{p}.imi") for p in POLICIES})
                        name = f"{image}-q{quality}"
                    else:
                        name = f"{image}-q{quality}-{policy}"
                        fields["codec"] = "imij-" + policy
                        inputs["encoded"] = str(files / f"{name}.imi")
                    cases.append({"id": name, "inputs": inputs, "fields": fields})
        arms = []
        for policy_or_threads in (POLICIES if action == "encode" else [1, 2, 4, 8]):
            if action == "encode":
                name = policy_or_threads
                args = [action, "imij-" + name, "${quality}", "1", "${raw}", "${" + name + "}"]
            else:
                name = str(policy_or_threads)
                args = [action, "${codec}", "${quality}", name, "${raw}", "${encoded}"]
            arms.append({"id": name, "program": str(probe), "args": args,
                         "cwd": str(root), "env": {},
                         "timeout_seconds": 30, "revision": identities[str(probe)]})
        pointers = ["/source_sha256", "/bytes", "/matches_prepared"] if action == "encode" else ["/sha256", "/bytes"]
        plan = {"schema": 1, "baseline": arms[0], "candidates": arms[1:], "cases": cases,
                "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
                           "unit": "ms", "stream": "stdout", "expected": 1, "divide_by": 1},
                "check": {"kind": "json", "prefix": "check=", "pointers": pointers, "stream": "stdout"},
                "warmup": "Encode: one warmup/one observation. Decode: three warmups/ten allocating decodes. Exact serial/threaded checks outside timing.",
                "blocks": 3, "aa": True, "deadline_seconds": 600,
                "settle_ms": 0, "seed": 731, "env": {}, "clean_env": False, "record_env": []}
        path = root / f"{action}-plan.json"
        path.write_text(json.dumps(plan, indent=2))
        execute([str(zenbench), "pair", str(path), "--output", str(root / action)], root / f"{action}.log")
        packet = json.loads(gzip.decompress((root / action / "packet.json.gz").read_bytes()))
        require(packet["complete"] and packet["plan"] == plan, "incomplete/changed plan")
        names = [a["id"] for a in arms] + ["__aa"]
        require(len(packet["invocations"]) == len(cases) * len(names) * 3, "incomplete observations")
        seen, cells, correct = set(), {}, {}
        for obs in packet["invocations"]:
            require(obs["status"] == "completed" and obs["exit_code"] == 0 and obs["continuation_safe"], "failed process")
            key = (obs["case"], obs["arm"], obs["block"])
            require(key not in seen and 0 <= key[0] < len(cases) and 0 <= key[1] < len(names) and 0 <= key[2] < 3, "invalid observation")
            seen.add(key)
            values = obs["values_ns"]
            require(len(values) == 1 and math.isfinite(values[0]) and values[0] > 0, "invalid timing")
            for filename, digest in obs["artifacts"].items():
                require(sha(pathlib.Path(filename)) == digest, "artifact mismatch")
            cells.setdefault((cases[key[0]]["id"], names[key[1]]), []).append(values[0] / 1e6)
            correct.setdefault((key[0], key[2]), []).append(obs["correctness"])
            if action == "encode":
                encode_ms += metric((pathlib.Path(obs["output"]) / "stdout.txt").read_text(), "encode_total_ms")
        require(all(len(v) == 3 for v in cells.values()), "missing repeats")
        require(all(all(v == group[0] for v in group) for group in correct.values()), "incorrect pixels/encode")
        require(all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]), "invalid summary")
        require(encode_ms < 60000, "additional encode budget exceeded")
        all_cells[action] = cells
        counts[action] = len(packet["invocations"])
        aa[action] = [s for s in packet["summaries"] if s["arm"] == "__aa"]
    rows = []
    for quality in QUALITIES:
        for policy in POLICIES:
            keys = [f"{i}-q{quality}-{policy}" for i in IMAGES]
            row = {"quality": quality, "policy": policy,
                   "entropy_by_image": [preparations[k]["entropy"] for k in keys],
                   "bytes_by_image": [preparations[k]["bytes"] for k in keys]}
            row["bits_per_pixel"] = 8 * sum(row["bytes_by_image"]) / (3 * PIXELS)
            times = [max(all_cells["encode"][(f"{i}-q{quality}", policy)]) for i in IMAGES]
            row["encode_ms_by_image"] = times
            row["encode_mpx_s"] = 3 * PIXELS / (1000 * sum(times))
            for threads in [1, 2, 4, 8]:
                times = [max(all_cells["decode"][(k, str(threads))]) for k in keys]
                row[f"decode_{threads}t_ms_by_image"] = times
                row[f"decode_{threads}t_mpx_s"] = 3 * PIXELS / (1000 * sum(times))
            rows.append(row)
    require(all(sha(pathlib.Path(p)) == h for p, h in identities.items()), "source/program changed")
    require(all(sha(files / f"{k}.imi") == v["encoded_sha256"] for k, v in preparations.items()), "fixture changed")
    summary = {"rows": rows, "aa": aa, "observations": counts,
               "preparation_encode_ms": preparation_ms, "all_encode_seconds": encode_ms / 1000}
    (root / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 6, "lossy_tiers.py PROBE INSPECTOR RAW ZENBENCH NEW-OUTPUT")
    run(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
