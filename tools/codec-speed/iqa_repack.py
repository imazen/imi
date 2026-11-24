"""Rebuild archive indexes from verified image payloads, without re-encoding images."""
import json
import pathlib
import sys
from fast_lossless import execute, require, sha


def repack(probe, study, output):
    source = study / "summary.json"
    report = json.loads(source.read_text())
    require(report["full_dataset"] and report["all_pixels_and_item_metadata_exact"], "incomplete source")
    output.mkdir()
    identity = {"source_summary_sha256": sha(source), "program_sha256": sha(probe)}
    (output / "identity.json").write_text(json.dumps(identity, indent=2))
    rows = [r for r in report["arms"] if not r["arm"].startswith("set-")]
    for arm, policy, payload_arm in [("set-independent", "plain", "independent"), ("set-reference", "indexed", "reference")]:
        packet = json.loads(execute([str(probe), "pack", str(study / f"{arm}.txt"),
                                    str(output / f"{arm}.imijset"), policy], output / f"{arm}.log"))
        require(packet["all_entries_identical"], "changed payload")
        require(packet["payload_bytes"] == next(r["bytes"] for r in rows if r["arm"] == payload_arm), "wrong payload total")
        require(packet["entries"] == (10206 if policy == "plain" else 81), "entry count")
        rows.append({"arm": arm, "bytes": packet["archive_bytes"],
                     "overhead_bytes": packet["archive_bytes"] - packet["payload_bytes"],
                     "sequence_index_bytes": packet["sequence_index_bytes"]})
    report["arms"] = rows
    report["archive_header_summaries_verified"] = True
    report["original_generation_summary_sha256"] = identity["source_summary_sha256"]
    require(sha(probe) == identity["program_sha256"] and sha(source) == identity["source_summary_sha256"], "identity changed")
    (output / "summary.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({k: v for k, v in report.items() if k != "groups"}, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 4, "iqa_repack.py PROBE VERIFIED-STUDY NEW-OUTPUT")
    repack(*(pathlib.Path(p).resolve() for p in sys.argv[1:]))
