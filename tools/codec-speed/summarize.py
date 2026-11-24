"""Audit PR33 packets and summarize the slowest of three fresh processes."""
import gzip
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve()
packets = {}
invocations = 0
aa = []
for path in sorted((root / "paired").glob("*/packet.json.gz")):
    p = json.loads(gzip.decompress(path.read_bytes()))
    assert p["complete"] and p["plan"]["blocks"] == 3 and p["plan"]["aa"]
    cases = p["plan"]["cases"]
    assert [c["id"] for c in cases] == ["1000", "1400", "1600"]
    names = [p["plan"]["baseline"]["id"]] + [a["id"] for a in p["plan"]["candidates"]] + ["__aa"]
    assert len(p["invocations"]) == 3 * 3 * len(names)
    cells = {}
    for run in p["invocations"]:
        assert run["status"] == "completed" and run["exit_code"] == 0 and run["continuation_safe"]
        assert len(run["values_ns"]) == 1 and run["values_ns"][0] > 0
        for file, expected in run["artifacts"].items():
            assert hashlib.sha256(pathlib.Path(file).read_bytes()).hexdigest() == expected
        key = (cases[run["case"]]["id"], names[run["arm"]])
        cells.setdefault(key, []).append(run["values_ns"][0] / 1e6)
        invocations += 1
    assert all(len(times) == 3 for times in cells.values())
    assert all(s["complete_blocks"] == 3 and s["correctness_checked"] for s in p["summaries"])
    for case in range(3):
        for block in range(3):
            runs = [r for r in p["invocations"] if r["case"] == case and r["block"] == block]
            assert len(runs) == len(names)
            assert all(r["correctness"] == runs[0]["correctness"] for r in runs)
    aa.extend({"plan": path.parent.name, **s} for s in p["summaries"] if s.get("arm") == "__aa")
    packets[path.parent.name] = (p, cells)
assert len(packets) == 9 and invocations == 297 and len(aa) == 27

encode_packet, encodes = packets["encode"]
encode_total_ms = 0.0
for run in encode_packet["invocations"]:
    lines = (pathlib.Path(run["output"]) / "stdout.txt").read_text().splitlines()
    values = [float(line.removeprefix("encode_total_ms=")) for line in lines if line.startswith("encode_total_ms=")]
    assert len(values) == 1
    encode_total_ms += values[0]
preparation_seconds = float((root / "results/complete.txt").read_text().strip().split("=")[1])
rows = []
for codec, setting in [("imij", 0), ("imij", 50), ("imij", 80), ("zenjpeg", 50), ("zenjpeg", 80), ("zenpng", 1), ("zenpng", 2), ("zenpng", 7)]:
    packet, cells = packets[f"decode-{codec}-{setting}"]
    threads = 2 if codec == "zenpng" else 8
    row = {"codec": codec, "setting": setting, "multi_threads": threads}
    for t in [1, threads]:
        worst = [max(cells[(image, f"{codec}_{setting}_{t}t")]) for image in ["1000", "1400", "1600"]]
        row[f"decode_{t}t_ms_by_image"] = worst
        row[f"decode_{t}t_mpx_s"] = (3 * 1024 * 1024 / 1e6) / (sum(worst) / 1000)
    row["encode_ms_by_image"] = [max(encodes[(i, f"{codec}_{setting}")]) for i in ["1000", "1400", "1600"]]
    row["encode_ms_mean"] = sum(row["encode_ms_by_image"]) / 3
    row["bytes_by_image"] = [(root / "results" / f"{i}-{codec}-{setting}.bin").stat().st_size for i in ["1000", "1400", "1600"]]
    row["mean_kib"] = sum(row["bytes_by_image"]) / 3 / 1024
    row["encode_mpx_s"] = (3 * 1024 * 1024 / 1e6) / (sum(row["encode_ms_by_image"]) / 1000)
    row["bits_per_pixel"] = 8 * sum(row["bytes_by_image"]) / (3 * 1024 * 1024)
    rows.append(row)
result = {"rows": rows, "invocations": invocations, "encode_seconds_including_preparation_warmups_aa": encode_total_ms/1000+preparation_seconds, "aa_summaries": aa}
output = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else root / "summary.json"
with output.open("x") as f:
    f.write(json.dumps(result, indent=2))
print("codec setting | mean KiB | bits/pixel | encode ms | encode MPx/s | decode 1t MPx/s | decode multi MPx/s")
for r in rows:
    print(f'{r["codec"]} {r["setting"]} | {r["mean_kib"]:.1f} | {r["bits_per_pixel"]:.3f} | {r["encode_ms_mean"]:.2f} | {r["encode_mpx_s"]:.1f} | {r["decode_1t_mpx_s"]:.0f} | {r[f"decode_{r["multi_threads"]}t_mpx_s"]:.0f}')
print("Total encode seconds:", result["encode_seconds_including_preparation_warmups_aa"])
