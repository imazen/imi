"""Frozen million-entry archives: paired listing CPU cost and whole-process RSS.

Caller holds the usual host lock and heavy-job resource scope. No build occurs.
"""
import gzip
import json
import math
import pathlib
import sys

from fast_lossless import execute, require, sha


def run(baseline, candidate, zenbench, output):
    output.mkdir()
    identities = {str(p): sha(p) for p in [baseline, candidate, zenbench]}
    cases = []
    for kind in ["plain", "filenames"]:
        archive = output / f"{kind}.imijset"
        execute(["env", f"IMIJ_SET_FILENAME_QUERY={int(kind == 'filenames')}",
                 str(baseline), str(archive), "1000000"], output / f"prepare-{kind}.log")
        identities[str(archive)] = sha(archive)
        cases.append({"id": kind, "inputs": {"archive": str(archive)}})
    arms = [{"id": name, "program": str(program),
             "args": ["--read", "${archive}", "1000000"],
             "timeout_seconds": 60, "revision": identities[str(program)]}
            for name, program in [("baseline", baseline), ("candidate", candidate)]]
    plan = {"schema": 1, "baseline": arms[0], "candidates": arms[1:], "cases": cases,
            "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
                       "unit": "ms", "expected": 1, "divide_by": 1},
            "check": {"kind": "json", "prefix": "check=",
                      "pointers": ["/entries", "/archive_bytes", "/listing_hash"]},
            "warmup": "Archive loaded before clock; three full listing warmups and one measured listing. Includes entry verification and rolling identity hash. Single thread.",
            "blocks": 3, "aa": True, "deadline_seconds": 180}
    path = output / "plan.json"
    path.write_text(json.dumps(plan, indent=2))
    (output / "identities.json").write_text(json.dumps(identities, indent=2))
    execute([str(zenbench), "pair", str(path), "--wait", "180", "--output", str(output / "paired")],
            output / "pair.log")
    packet = json.loads(gzip.decompress((output / "paired/packet.json.gz").read_bytes()))
    require(packet["complete"] and len(packet["invocations"]) == 18, "incomplete pair")
    names = ["baseline", "candidate", "__aa"]
    values, checks, unique = {}, {}, set()
    for obs in packet["invocations"]:
        require(obs["status"] == "completed" and obs["exit_code"] == 0 and obs["continuation_safe"], "failed observation")
        key = (obs["case"], obs["arm"], obs["block"])
        require(key not in unique and 0 <= key[0] < 2 and 0 <= key[1] < 3 and 0 <= key[2] < 3, "observation identity")
        unique.add(key)
        require(len(obs["values_ns"]) == 1, "metric count")
        ms = obs["values_ns"][0] / 1e6
        require(math.isfinite(ms) and ms > 0, "invalid metric")
        values.setdefault((key[0], key[1]), []).append(ms)
        checks.setdefault(key[0], []).append(obs["correctness"])
        for filename, digest in obs["artifacts"].items():
            require(sha(pathlib.Path(filename)) == digest, "observation artifact changed")
    require(all(all(c == group[0] for c in group) for group in checks.values()), "listing identities differ")
    require(all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]), "incomplete checked summary")
    rows = []
    for case, spec in enumerate(cases):
        for arm, name in enumerate(names):
            times = values[(case, arm)]
            rows.append({"case": spec["id"], "arm": name, "milliseconds": times, "worst_ms": max(times)})
        for name, program in [("baseline", baseline), ("candidate", candidate)]:
            execute(["/usr/bin/time", "-v", str(program), "--read", spec["inputs"]["archive"], "1000000"],
                    output / f"rss-{spec['id']}-{name}.log")
    require(all(sha(pathlib.Path(p)) == digest for p, digest in identities.items()), "input/program changed")
    (output / "summary.json").write_text(json.dumps(rows, indent=2))
    print(json.dumps(rows, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 5, "set_listing_pair.py BASELINE CANDIDATE ZENBENCH NEW-OUTPUT")
    run(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
