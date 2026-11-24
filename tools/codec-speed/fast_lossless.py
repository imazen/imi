"""Bounded QOI/IMIJ lossless comparison with the existing PR33 process probe."""
import gzip
import hashlib
import json
import math
import pathlib
import shutil
import subprocess
import sys

IMAGES = ["1000", "1400", "1600"]
SETTINGS = [("qoi", "0"), ("imij-lossless", "0"),
            ("imij-lossless", "2.5"), ("imij-lossless", "7.5")]
PIXELS = 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def metric(output, name):
    values = [float(line.split("=", 1)[1]) for line in output.splitlines()
              if line.startswith(name + "=")]
    require(len(values) == 1 and math.isfinite(values[0]) and values[0] >= 0, name)
    return values[0]


def execute(argv, output):
    with output.with_suffix(".command.json").open("x") as file:
        json.dump(argv, file, indent=2)
    with output.open("x") as file:
        result = subprocess.run(argv, stdout=file, stderr=subprocess.STDOUT, check=False)
    output.with_suffix(".rc").write_text(str(result.returncode) + "\n")
    require(result.returncode == 0, f"command failed: {output}")
    return output.read_text()


def run(probe, raw, zenbench, root):
    root.mkdir()
    fixtures = root / "results"
    fixtures.mkdir()
    plans = root / "plans"
    plans.mkdir()
    paired = root / "paired"
    paired.mkdir()
    identities = {"probe_sha256": sha(probe), "zenbench_sha256": sha(zenbench), "inputs": {}}
    encode_ms = 0.0
    for image in IMAGES:
        source = raw / f"{image}.rgb"
        require(source.stat().st_size == PIXELS * 3, "expected fixed RGB8 crop")
        identities["inputs"][image] = sha(source)
        shutil.copyfile(source, fixtures / source.name)
        for codec, effort in SETTINGS:
            require(encode_ms < 60_000, "new encode budget exhausted")
            target = fixtures / f"{image}-{codec}-{effort}.bin"
            output = execute([str(probe), "prepare", codec, effort, "1",
                              str(fixtures / source.name), str(target)],
                             root / f"prepare-{image}-{codec}-{effort}.log")
            encode_ms += metric(output, "encode_total_ms")
    (root / "identities.json").write_text(json.dumps(identities, indent=2))
    preparations_ms = encode_ms

    def arm(codec, effort, threads, action):
        return {"id": f"{codec}_{effort}_{threads}t", "program": str(probe),
                "args": [action, codec, effort, str(threads), "${raw}",
                         "${" + f"{codec}_{effort}" + "}"],
                "timeout_seconds": 30, "revision": identities["probe_sha256"]}

    cases = []
    for image in IMAGES:
        inputs = {"raw": str(fixtures / f"{image}.rgb")}
        inputs.update({f"{c}_{e}": str(fixtures / f"{image}-{c}-{e}.bin") for c, e in SETTINGS})
        cases.append({"id": image, "inputs": inputs})
    packets = {}
    for action in ["encode", "decode"]:
        arms = [arm(c, e, t, action) for c, e in SETTINGS
                for t in ([1] if action == "encode" or c == "qoi" else [1, 2, 4, 8])]
        checks = ["/source_sha256", "/bytes", "/matches_prepared"] if action == "encode" else ["/sha256", "/bytes"]
        plan = {"schema": 1, "baseline": arms[0], "candidates": arms[1:], "cases": cases,
                "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
                           "unit": "ms", "expected": 1, "divide_by": 1},
                "check": {"kind": "json", "prefix": "check=", "pointers": checks},
                "warmup": "One encode warmup then one encode; three decode warmups then ten allocating decodes per process. Exact raw/fixture checks outside timing.",
                "blocks": 3, "aa": True, "deadline_seconds": 180}
        path = plans / f"{action}.json"
        path.write_text(json.dumps(plan, indent=2))
        execute([str(zenbench), "pair", str(path), "--output", str(paired / action)], root / f"{action}.log")
        packet = json.loads(gzip.decompress((paired / action / "packet.json.gz").read_bytes()))
        require(packet["complete"] and packet["plan"]["blocks"] == 3 and packet["plan"]["aa"], "incomplete packet")
        names = [a["id"] for a in arms] + ["__aa"]
        require(len(packet["invocations"]) == 9 * len(names), "wrong observation count")
        cells, unique, groups = {}, set(), {}
        for observation in packet["invocations"]:
            require(observation["status"] == "completed" and observation["exit_code"] == 0
                    and observation["continuation_safe"], "failed observation")
            values = observation["values_ns"]
            require(len(values) == 1 and math.isfinite(values[0]) and values[0] > 0, "invalid time")
            key = (observation["case"], observation["arm"], observation["block"])
            require(key not in unique and 0 <= key[0] < 3 and 0 <= key[1] < len(names) and 0 <= key[2] < 3, "duplicate/outside observation")
            unique.add(key)
            groups.setdefault((key[0], key[2]), []).append(observation["correctness"])
            for filename, expected in observation["artifacts"].items():
                require(sha(pathlib.Path(filename)) == expected, "artifact hash mismatch")
            cells.setdefault((IMAGES[key[0]], names[key[1]]), []).append(values[0] / 1e6)
            if action == "encode":
                encode_ms += metric((pathlib.Path(observation["output"]) / "stdout.txt").read_text(), "encode_total_ms")
        require(all(len(x) == 3 for x in cells.values()), "missing repeats")
        require(all(all(v == group[0] for v in group) for group in groups.values()), "correctness mismatch")
        require(all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in packet["summaries"]), "incomplete summary")
        packets[action] = (packet, cells)
        require(encode_ms < 60_000, "new encode budget exceeded")
    require(sha(probe) == identities["probe_sha256"] and sha(zenbench) == identities["zenbench_sha256"], "program changed")
    require(all(sha(raw / f"{i}.rgb") == identities["inputs"][i] for i in IMAGES), "source changed")
    rows = []
    for codec, effort in SETTINGS:
        row = {"codec": codec, "effort": float(effort) if codec != "qoi" else None}
        row["bytes_by_image"] = [(fixtures / f"{i}-{codec}-{effort}.bin").stat().st_size for i in IMAGES]
        row["bits_per_pixel"] = 8 * sum(row["bytes_by_image"]) / (3 * PIXELS)
        times = [max(packets["encode"][1][(i, f"{codec}_{effort}_1t")]) for i in IMAGES]
        row["encode_ms_by_image"] = times
        row["encode_mpx_s"] = 3 * PIXELS / (1000 * sum(times))
        for threads in ([1] if codec == "qoi" else [1, 2, 4, 8]):
            times = [max(packets["decode"][1][(i, f"{codec}_{effort}_{threads}t")]) for i in IMAGES]
            row[f"decode_{threads}t_ms_by_image"] = times
            row[f"decode_{threads}t_mpx_s"] = 3 * PIXELS / (1000 * sum(times))
        rows.append(row)
    result = {"rows": rows, "preparation_encode_ms": preparations_ms,
              "all_encode_seconds": encode_ms / 1000,
              "invocations": sum(len(p[0]["invocations"]) for p in packets.values()),
              "aa_summaries": {a: [s for s in p[0]["summaries"] if s["arm"] == "__aa"] for a, p in packets.items()}}
    (root / "summary.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 5, "fast_lossless.py PROBE RAW-CROPS ZENBENCH NEW-OUTPUT")
    run(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
