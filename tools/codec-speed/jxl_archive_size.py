"""Encode the exact KADID archive inputs with one lossless ZenJXL profile.

The JSONL journal makes a long run resumable without silently accepting partial
outputs. Elapsed times are diagnostics; this tool compares stored bytes.
"""

import argparse
import csv
import hashlib
import json
import os
import pathlib
import subprocess
import time


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_rows(
    group: str, source_dir: pathlib.Path
) -> tuple[dict[str, list[str]], str]:
    table = source_dir / group / "source.tsv"
    with table.open(newline="") as stream:
        rows = list(csv.reader(stream, delimiter="\t"))
    require(
        len(rows) == 126 and all(len(row) == 7 for row in rows),
        f"bad source table: {table}",
    )
    indexed = {row[0]: row for row in rows}
    require(len(indexed) == 126, f"duplicate source name: {table}")
    return indexed, sha256(table)


def write_json(path: pathlib.Path, value: object) -> None:
    temporary = path.with_name(path.name + ".partial")
    require(not temporary.exists(), f"partial record needs inspection: {temporary}")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def run(args: argparse.Namespace) -> None:
    manifest = json.loads(args.manifest.read_text())
    require(
        manifest["references"] == 81 and manifest["distortions"] == 10125,
        "wrong KADID manifest",
    )
    groups = manifest["groups"]
    if args.groups:
        requested = args.groups.split(",")
        require(len(requested) == len(set(requested)), "duplicate requested group")
        groups = [
            group
            for group in groups
            if pathlib.Path(group["reference"]).stem in requested
        ]
        require(len(groups) == len(requested), "missing requested group")
    require(
        groups and all(len(group["items"]) == 126 for group in groups),
        "incomplete groups",
    )
    profile_tier = {"max": 0, "fast-decode": 4}[args.profile]
    require(args.encoder.is_file(), "missing encoder")
    tables = {
        pathlib.Path(group["reference"]).stem: source_rows(
            pathlib.Path(group["reference"]).stem, args.source_dir
        )
        for group in groups
    }
    plan = {
        "profile": args.profile,
        "effort": 13,
        "faster_decoding": profile_tier,
        "exact_decoded_samples_intended": True,
        "driver_sha256": sha256(pathlib.Path(__file__)),
        "encoder_sha256": sha256(args.encoder),
        "input_manifest_sha256": sha256(args.manifest),
        "source_tables_sha256": {
            name: table_sha for name, (_, table_sha) in tables.items()
        },
        "groups": list(tables),
        "images": sum(len(group["items"]) for group in groups),
        "scope": "individual lossless JXL files; no shared reference prediction or archive metadata",
    }
    args.output.mkdir(parents=True, exist_ok=True)
    plan_path = args.output / "plan.json"
    if plan_path.exists():
        require(json.loads(plan_path.read_text()) == plan, "resume plan changed")
    else:
        require(not any(args.output.iterdir()), "nonempty output without plan")
        write_json(plan_path, plan)
    journal = args.output / "completed.jsonl"
    completed: dict[str, dict] = {}
    if journal.exists():
        for line in journal.read_text().splitlines():
            record = json.loads(line)
            require(record["filename"] not in completed, "duplicate completion record")
            completed[record["filename"]] = record
    require(len(completed) <= plan["images"], "too many completed records")
    expected_names: set[str] = set()
    observed_records = 0
    total_png = total_jxl = total_pixels = 0
    with journal.open("a") as records:
        for group in groups:
            group_name = pathlib.Path(group["reference"]).stem
            expected, _ = tables[group_name]
            require(
                set(expected) == {item["filename"] for item in group["items"]},
                "table/group mismatch",
            )
            for item in group["items"]:
                filename = item["filename"]
                require(filename not in expected_names, f"duplicate image: {filename}")
                expected_names.add(filename)
                source = args.images / filename
                table_row = expected[filename]
                require(
                    source.stat().st_size == int(table_row[1]) == item["encoded_bytes"],
                    "PNG length changed",
                )
                source_hash = sha256(source)
                require(source_hash == table_row[2], f"PNG content changed: {filename}")
                require(
                    int(table_row[5]) == item["width"]
                    and int(table_row[6]) == item["height"],
                    "image size changed",
                )
                output = args.output / (pathlib.Path(filename).stem + ".jxl")
                if filename in completed:
                    old = completed[filename]
                    require(
                        old["source_sha256"] == source_hash, "completed source changed"
                    )
                    require(
                        old["group"] == group_name
                        and old["png_bytes"] == source.stat().st_size
                        and old["pixels"] == item["width"] * item["height"]
                        and old["returncode"] == 0,
                        "completed record changed",
                    )
                    require(
                        output.stat().st_size == old["jxl_bytes"]
                        and sha256(output) == old["jxl_sha256"],
                        "completed JXL changed",
                    )
                    record = old
                else:
                    partial = output.with_name(output.name + ".partial")
                    log = args.output / (pathlib.Path(filename).stem + ".log")
                    require(
                        not output.exists()
                        and not partial.exists()
                        and not log.exists(),
                        f"unrecorded output needs inspection: {filename}",
                    )
                    command = [
                        str(args.encoder),
                        str(source),
                        str(partial),
                        "--lossless",
                        "--effort",
                        "13",
                        "--faster-decoding",
                        str(profile_tier),
                        "--quiet",
                    ]
                    start = time.monotonic_ns()
                    with log.open("wb") as stream:
                        result = subprocess.run(
                            command,
                            stdout=stream,
                            stderr=subprocess.STDOUT,
                            check=False,
                        )
                    elapsed_ns = time.monotonic_ns() - start
                    require(
                        result.returncode == 0,
                        f"encoder rc={result.returncode}: {filename} ({log})",
                    )
                    require(partial.stat().st_size > 0, f"empty JXL: {filename}")
                    partial.replace(output)
                    record = {
                        "filename": filename,
                        "group": group_name,
                        "source_sha256": source_hash,
                        "png_bytes": source.stat().st_size,
                        "jxl_sha256": sha256(output),
                        "jxl_bytes": output.stat().st_size,
                        "pixels": item["width"] * item["height"],
                        "elapsed_ns_diagnostic": elapsed_ns,
                        "command": command,
                        "returncode": result.returncode,
                    }
                    records.write(json.dumps(record, sort_keys=True) + "\n")
                    records.flush()
                    os.fsync(records.fileno())
                observed_records += 1
                total_png += record["png_bytes"]
                total_jxl += record["jxl_bytes"]
                total_pixels += record["pixels"]
            print(
                f"completed {group_name}: {len(expected_names)}/{plan['images']}",
                flush=True,
            )
    require(set(completed).issubset(expected_names), "journal has unexpected images")
    require(
        len(expected_names) == observed_records == plan["images"],
        "incomplete output",
    )
    require(
        sha256(args.encoder) == plan["encoder_sha256"]
        and sha256(args.manifest) == plan["input_manifest_sha256"],
        "program or manifest changed",
    )
    require(sha256(pathlib.Path(__file__)) == plan["driver_sha256"], "driver changed")
    require(
        all(
            sha256(args.source_dir / name / "source.tsv") == digest
            for name, digest in plan["source_tables_sha256"].items()
        ),
        "source table changed",
    )
    summary = {
        "profile": args.profile,
        "images": len(expected_names),
        "groups": len(groups),
        "pixels": total_pixels,
        "original_png_bytes": total_png,
        "jxl_payload_bytes": total_jxl,
        "encoder_sha256": plan["encoder_sha256"],
        "journal_sha256": sha256(journal),
        "source_manifest_sha256": plan["input_manifest_sha256"],
        "pixel_oracle": "not run by this size collector",
    }
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("encoder", type=pathlib.Path)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("source_dir", type=pathlib.Path)
    parser.add_argument("images", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--profile", required=True, choices=["max", "fast-decode"])
    parser.add_argument(
        "--groups", help="comma-separated pristine IDs for a declared subset"
    )
    run(parser.parse_args())
