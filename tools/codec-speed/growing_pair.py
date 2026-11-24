"""Admission-fix cost control using the existing generated-raster group probe.

This is not a photographic compression/speed evaluation. Run under the usual
host lock and resource wrapper; every output and scratch file is retained.
"""
import gzip
import json
import pathlib
import sys
import uuid

from fast_lossless import execute, require, sha


def measure(probe, output):
    run = output / uuid.uuid4().hex
    run.mkdir()
    image = run / "image.imi"
    stdout = execute(["/usr/bin/time", "-v", str(probe), "--growing", "4096", "4096",
                      str(image), str(run / "data"), str(run / "index")], run / "encode.log")
    metrics = [dict(field.split("=", 1) for field in line.split())
               for line in stdout.splitlines() if line.startswith("mode=growing ")]
    require(len(metrics) == 1, "missing encoder measurement")
    row = metrics[0]
    require(row["width"] == row["height"] == "4096", "geometry")
    execute([str(probe), "--verify", "4096", "4096", str(image)], run / "pixels.log")
    print(f"elapsed_ms={float(row['seconds']) * 1000:.9f}")
    print("check=" + json.dumps({"bytes": image.stat().st_size, "sha256": sha(image), "pixels_exact": True}))


def run(baseline, candidate, zenbench, output):
    output.mkdir()
    script = pathlib.Path(__file__).resolve()
    identities = {str(p): sha(p) for p in [baseline, candidate, zenbench, script]}
    arms = [{"id": name, "program": sys.executable,
             "args": [str(script), "--measure", str(probe), str(output)],
             "timeout_seconds": 90, "revision": identities[str(probe)]}
            for name, probe in [("baseline", baseline), ("candidate", candidate)]]
    plan = {"schema": 1, "baseline": arms[0], "candidates": arms[1:],
            "cases": [{"id": "generated4096", "inputs": {}}],
            "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
                       "unit": "ms", "expected": 1, "divide_by": 1},
            "check": {"kind": "json", "prefix": "check=", "pointers": ["/bytes", "/sha256", "/pixels_exact"]},
            "warmup": "No warmup. Fresh-process generated RGB8 raster; reported encoder interval includes source generation, scratch writes/replay and output flush. Readback/hash outside clock.",
            "blocks": 3, "aa": True, "deadline_seconds": 180}
    path = output / "plan.json"
    path.write_text(json.dumps(plan, indent=2))
    (output / "identities.json").write_text(json.dumps(identities, indent=2))
    execute([str(zenbench), "pair", str(path), "--wait", "180", "--output", str(output / "paired")], output / "pair.log")
    packet = json.loads(gzip.decompress((output / "paired/packet.json.gz").read_bytes()))
    require(packet["complete"] and len(packet["invocations"]) == 9, "incomplete pair")
    require(all(i["status"] == "completed" and i["exit_code"] == 0 and i["continuation_safe"] for i in packet["invocations"]), "failed process")
    require(all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]), "unchecked summary")
    require(all(sha(pathlib.Path(p)) == digest for p, digest in identities.items()), "identity changed")
    print(json.dumps(packet["summaries"], indent=2))


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--measure":
        measure(*(pathlib.Path(p).resolve() for p in sys.argv[2:]))
    else:
        require(len(sys.argv) == 5, "growing_pair.py BASELINE CANDIDATE ZENBENCH NEW-OUTPUT")
        run(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
