"""Three paired process rounds for exact, archive-backed KADID I01 access."""

import gzip
import hashlib
import json
import math
import pathlib
import statistics
import sys

from fast_lossless import execute, metric, require, sha


def file_sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(probe, independent, reference, group, pngs, zenbench, output, mode="archive-access", candidate=None):
    output.mkdir()
    require(mode in ["archive-access", "archive-batch", "binary-pair", "binary-batch-pair"], "access mode")
    require(mode not in ("binary-pair", "binary-batch-pair") or candidate is not None, "candidate program")
    inputs = [probe, independent, reference, group / "items.tsv", zenbench]
    if candidate is not None:
        inputs.append(candidate)
    for line in (group / "items.tsv").read_text().splitlines():
        name = line.split("\t", 1)[0]
        require(name and all(ch.isalnum() or ch in "_." for ch in name), "source filename")
        inputs.append(pngs / name)
    (output / "identities.json").write_text(
        json.dumps({str(path): file_sha(path) for path in inputs}, indent=2)
    )
    arms = []
    policies = [(independent, "independent"), (reference, "reference")]
    if mode == "archive-batch":
        policies.insert(1, (independent, "independent-list"))
    if mode in ("binary-pair", "binary-batch-pair"):
        policies = [(reference, "baseline"), (reference, "candidate")]
    for archive, name in policies:
        for threads in [1, 2, 4, 8]:
            program = candidate if name == "candidate" else probe
            arms.append(
                {
                    "id": f"{name}_{threads}t",
                    "program": str(program),
                    "args": [
                        "archive-batch" if mode == "binary-batch-pair" else
                        "archive-access" if mode == "binary-pair" else mode,
                        str(archive),
                        str(group),
                        str(pngs),
                        "reference" if mode in ("binary-pair", "binary-batch-pair") else name,
                        str(threads),
                    ],
                    "timeout_seconds": 120,
                    "revision": file_sha(program),
                }
            )
    plan = {
        "schema": 1,
        "baseline": arms[0],
        "candidates": arms[1:],
        "cases": [{"id": "I01-level-five-batch" if mode in ("archive-batch", "binary-batch-pair") else "I01-last-image", "inputs": {}}],
        "metric": {
            "extract": {"kind": "prefix", "prefix": "elapsed_ms="},
            "unit": "ms",
            "expected": 1,
            "divide_by": 1,
        },
        "check": {
            "kind": "json",
            "prefix": "check=",
            "pointers": [
                "/sha256",
                "/images",
                "/pixels_exact",
                "/archive_digest_verified",
            ],
        },
        "warmup": "Three full local archive accesses per process, then one timed access. Each opens the archive, looks up and verifies indexed payloads, decodes declared references, and checks original pixels. Batch mode retains one group and pristine raster for 25 requested distortions. PNG source loading and pixel assertions are outside timed phases. OS page cache is warm; network range latency is excluded.",
        "blocks": 3,
        "aa": True,
        "deadline_seconds": 180,
    }
    (output / "plan.json").write_text(json.dumps(plan, indent=2))
    execute(
        [
            str(zenbench),
            "pair",
            str(output / "plan.json"),
            "--wait",
            "180",
            "--output",
            str(output / "paired"),
        ],
        output / "run.log",
    )
    packet = json.loads(
        gzip.decompress((output / "paired" / "packet.json.gz").read_bytes())
    )
    require(packet["complete"] and packet["plan"]["blocks"] == 3, "incomplete pair")
    names = [arm["id"] for arm in arms] + ["__aa"]
    require(len(packet["invocations"]) == 3 * len(names), "observation count")
    values = {name: [] for name in names}
    seen = set()
    for obs in packet["invocations"]:
        require(
            obs["status"] == "completed"
            and obs["exit_code"] == 0
            and obs["continuation_safe"],
            "failed process",
        )
        key = (obs["case"], obs["arm"], obs["block"])
        require(
            key not in seen and key[0] == 0 and 0 <= key[1] < len(names)
            and 0 <= key[2] < 3,
            "observation identity",
        )
        seen.add(key)
        require(
            len(obs["values_ns"]) == 1
            and math.isfinite(obs["values_ns"][0])
            and obs["values_ns"][0] > 0,
            "invalid time",
        )
        for filename, digest in obs["artifacts"].items():
            require(sha(pathlib.Path(filename)) == digest, "artifact changed")
        stdout = (pathlib.Path(obs["output"]) / "stdout.txt").read_text()
        fields = {
            field: metric(stdout, field)
            for field in [
                "elapsed_ms", "lookup_ms", "copy_ms", "decode_ms",
                "decoded_images", "copied_bytes",
            ]
        }
        require(
            abs(fields["elapsed_ms"] - sum(fields[k] for k in ["lookup_ms", "copy_ms", "decode_ms"])) < 1e-7,
            "phase sum",
        )
        name = names[key[1]]
        if mode in ("archive-access", "binary-pair"):
            expect = 2 if mode == "binary-pair" or name.startswith("reference") else 1
            require(fields["decoded_images"] == expect, "dependency count")
        else:
            expect = 26 if mode == "binary-batch-pair" or name.startswith("reference") else 25
            require(fields["decoded_images"] == expect, "batch dependency count")
        values[name].append(fields)
    require(all(len(v) == 3 for v in values.values()), "missing round")
    require(
        all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]),
        "summary coverage",
    )
    require(
        all(file_sha(path) == digest for path, digest in
            ((pathlib.Path(name), value) for name, value in
             json.loads((output / "identities.json").read_text()).items())),
        "input changed",
    )
    summary = {
        "scope": f"warm-cache local archive, I01 {mode}, three paired process rounds",
        "rows": [
            {
                "arm": name,
                "median_ms": statistics.median(row["elapsed_ms"] for row in samples),
                "worst_ms": max(row["elapsed_ms"] for row in samples),
                "observations": samples,
            }
            for name, samples in values.items()
        ],
    }
    (output / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    require(
        len(sys.argv) == 8
        or (len(sys.argv) == 9 and sys.argv[8] == "batch")
        or (len(sys.argv) == 9 and sys.argv[8].startswith(("candidate=", "candidate-batch="))),
        "iqa_archive_timing.py PROBE INDEPENDENT-ARCHIVE REFERENCE-ARCHIVE I01-GROUP PNG-DIR ZENBENCH NEW-OUTPUT [batch|candidate=PROGRAM|candidate-batch=PROGRAM]",
    )
    candidate = None
    if len(sys.argv) == 9 and sys.argv[8].startswith(("candidate=", "candidate-batch=")):
        candidate = pathlib.Path(sys.argv[8].split("=", 1)[1]).resolve()
    run(
        *(pathlib.Path(arg).resolve() for arg in sys.argv[1:8]),
        mode="archive-batch" if len(sys.argv) == 9 and sys.argv[8] == "batch" else
        "binary-batch-pair" if len(sys.argv) == 9 and sys.argv[8].startswith("candidate-batch=") else
        "binary-pair" if candidate is not None else "archive-access",
        candidate=candidate,
    )
