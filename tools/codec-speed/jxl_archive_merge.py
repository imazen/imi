"""Verify and total disjoint ZenJXL KADID groups from bounded encode runs."""

import argparse
import csv
import hashlib
import json
import pathlib


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(args: argparse.Namespace) -> None:
    manifest = json.loads(args.manifest.read_text())
    require(
        manifest["references"] == 81 and manifest["distortions"] == 10125,
        "wrong KADID inventory",
    )
    groups = {
        pathlib.Path(group["reference"]).stem: group for group in manifest["groups"]
    }
    require(len(groups) == 81, "duplicate group")
    owners = {}
    root_summaries = {}
    for root in args.collections:
        plan = json.loads((root / "plan.json").read_text())
        summary = json.loads((root / "summary.json").read_text())
        require(
            plan["profile"] == summary["profile"] == "max"
            and plan["effort"] == summary["effort"] == 13
            and plan["faster_decoding"] == summary["faster_decoding"] == 0
            and plan["manifest_sha256"]
            == summary["source_manifest_sha256"]
            == sha256(args.manifest)
            and plan["encoder_sha256"] == summary["encoder_sha256"],
            f"root plan/summary mismatch: {root}",
        )
        require(
            len(plan["groups"]) == summary["groups"]
            and summary["images"] == 126 * summary["groups"]
            and set(summary["group_summary_sha256"]) == set(plan["groups"]),
            f"root group count mismatch: {root}",
        )
        for group in plan["groups"]:
            require(
                group in groups and group not in owners,
                f"duplicate/unknown group: {group}",
            )
            owners[group] = root
        root_summaries[root] = summary
    require(set(owners) == set(groups), "missing group")
    require(
        len({s["encoder_sha256"] for s in root_summaries.values()}) == 1,
        "different encoders",
    )
    payload = png_bytes = pixels = 0
    file_lines = []
    group_hashes = {}
    for group_name, group in groups.items():
        root = owners[group_name]
        collection = root / "groups" / group_name
        summary_path = collection / "summary.json"
        summary = json.loads(summary_path.read_text())
        group_plan = json.loads((collection / "plan.json").read_text())
        journal_path = collection / "completed.jsonl"
        source_table = args.item_tables / group_name / "source.tsv"
        require(
            root_summaries[root]["group_summary_sha256"][group_name]
            == sha256(summary_path),
            f"group summary changed: {group_name}",
        )
        require(
            summary["profile"] == "max"
            and summary["groups"] == 1
            and summary["images"] == 126
            and summary["encoder_sha256"] == root_summaries[root]["encoder_sha256"]
            and summary["journal_sha256"] == sha256(journal_path),
            f"bad group summary: {group_name}",
        )
        require(
            group_plan["groups"] == [group_name]
            and group_plan["profile"] == "max"
            and group_plan["encoder_sha256"] == summary["encoder_sha256"]
            and group_plan["input_manifest_sha256"] == sha256(args.manifest)
            and group_plan["source_tables_sha256"]
            == {group_name: sha256(source_table)},
            f"bad group plan: {group_name}",
        )
        entries = [json.loads(line) for line in journal_path.read_text().splitlines()]
        require(len(entries) == len(group["items"]) == 126, "bad group length")
        table = args.item_tables / f"{group_name}.tsv"
        with table.open(newline="") as stream:
            attributes = list(csv.reader(stream, delimiter="\t"))
        with source_table.open(newline="") as stream:
            sources = list(csv.reader(stream, delimiter="\t"))
        require(len(attributes) == 126, f"bad metadata table: {group_name}")
        require(len(sources) == 126, f"bad source table: {group_name}")
        group_payload = group_png = group_pixels = 0
        for item, record, row, source in zip(
            group["items"], entries, attributes, sources, strict=True
        ):
            filename = item["filename"]
            require(
                pathlib.Path(filename).name == filename
                and filename.endswith(".png")
                and len(row) == 7
                and row[0] == filename == record["filename"]
                and len(source) == 7
                and source[0] == filename
                and int(source[1]) == record["png_bytes"]
                and source[2] == record["source_sha256"]
                and int(source[5]) == item["width"]
                and int(source[6]) == item["height"],
                f"item order changed: {filename}",
            )
            encoded = collection / (pathlib.Path(filename).stem + ".jxl")
            require(
                encoded.stat().st_size == record["jxl_bytes"]
                and sha256(encoded) == record["jxl_sha256"]
                and item["encoded_bytes"] == record["png_bytes"]
                and item["width"] * item["height"] == record["pixels"]
                and record["returncode"] == 0,
                f"JXL/source mismatch: {filename}",
            )
            file_lines.append(
                "\t".join(
                    [str(encoded), str(record["jxl_bytes"]), record["jxl_sha256"]]
                    + row
                    + source[3:]
                )
            )
            group_payload += record["jxl_bytes"]
            group_png += record["png_bytes"]
            group_pixels += record["pixels"]
        require(
            (group_payload, group_png, group_pixels)
            == (
                summary["jxl_payload_bytes"],
                summary["original_png_bytes"],
                summary["pixels"],
            ),
            f"bad group totals: {group_name}",
        )
        payload += group_payload
        png_bytes += group_png
        pixels += group_pixels
        group_hashes[group_name] = sha256(summary_path)
    require(
        (payload, png_bytes, pixels)
        == tuple(
            sum(summary[key] for summary in root_summaries.values())
            for key in ("jxl_payload_bytes", "original_png_bytes", "pixels")
        ),
        "root totals disagree",
    )
    require(len(file_lines) == 10206, "incomplete file list")
    args.output.mkdir(parents=True, exist_ok=False)
    file_list = args.output / "files.tsv"
    file_list.write_text("\n".join(file_lines) + "\n")
    merged = {
        "profile": "max",
        "effort": 13,
        "faster_decoding": 0,
        "groups": 81,
        "images": len(file_lines),
        "pixels": pixels,
        "original_png_bytes": png_bytes,
        "jxl_payload_bytes": payload,
        "encoder_sha256": next(iter(root_summaries.values()))["encoder_sha256"],
        "source_manifest_sha256": sha256(args.manifest),
        "file_list_sha256": sha256(file_list),
        "group_summary_sha256": group_hashes,
        "scope": "individual lossless JXL payload bytes; no archive index or metadata",
    }
    (args.output / "summary.json").write_text(
        json.dumps(merged, indent=2, sort_keys=True) + "\n"
    )
    print(json.dumps(merged, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("item_tables", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("collections", nargs=2, type=pathlib.Path)
    run(parser.parse_args())
