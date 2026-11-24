"""Decode a merged lossless JXL collection against original KADID RGB8 samples."""

import argparse
import csv
import hashlib
import json
import os
import pathlib
import subprocess

from PIL import Image

from jxl_archive_oracle import require, sha256, write_json


def load_items(
    manifest_path: pathlib.Path,
    sources: pathlib.Path,
    collection: pathlib.Path,
    allow_subset: bool,
) -> tuple[dict, list[dict], dict[str, str]]:
    manifest = json.loads(manifest_path.read_text())
    summary = json.loads((collection / "summary.json").read_text())
    require(
        summary["source_manifest_sha256"] == sha256(manifest_path),
        "source manifest changed",
    )
    listing = collection / "files.tsv"
    require(summary["file_list_sha256"] == sha256(listing), "file list changed")
    groups = manifest["groups"]
    require(len(groups) == 81, "source group count changed")
    names = [pathlib.Path(group["reference"]).stem for group in groups]
    require(len(set(names)) == 81, "duplicate source group")
    rows = [line.split("\t") for line in listing.read_text().splitlines()]
    require(len(rows) == summary["images"], "incomplete merged inventory")
    require(
        allow_subset or len(rows) == 10206 == 126 * 81,
        "full merged inventory required",
    )
    selected = set(summary["group_summary_sha256"])
    require(selected.issubset(names), "unexpected merged group")
    require(allow_subset or selected == set(names), "missing merged group")
    expected = []
    table_hashes = {}
    for group, name in zip(groups, names, strict=True):
        if name not in selected:
            continue
        table = sources / name / "source.tsv"
        table_hashes[name] = sha256(table)
        with table.open(newline="") as stream:
            source_rows = list(csv.reader(stream, delimiter="\t"))
        require(len(source_rows) == len(group["items"]) == 126, "source table length")
        for item, source in zip(group["items"], source_rows, strict=True):
            require(
                len(source) == 7
                and source[0] == item["filename"]
                and int(source[1]) == item["encoded_bytes"]
                and int(source[5]) == item["width"]
                and int(source[6]) == item["height"],
                f"source table differs: {name}",
            )
            expected.append((name, item, source))
    require(len(rows) == len(expected), "merged source coverage")
    require(
        summary["groups"] == len(selected)
        and summary["pixels"] == sum(i["width"] * i["height"] for _, i, _ in expected),
        "merged geometry total changed",
    )
    items = []
    payload_bytes = 0
    for row, (name, item, source) in zip(rows, expected, strict=True):
        filename = item["filename"]
        require(
            len(row) == 14 and row[3] == filename and row[10:14] == source[3:7],
            f"merged item differs: {filename}",
        )
        jxl = pathlib.Path(row[0])
        require(
            jxl.is_absolute()
            and jxl.parent.name == name
            and jxl.name == pathlib.Path(filename).stem + ".jxl",
            f"merged JXL path differs: {filename}",
        )
        size = int(row[1])
        require(size > 0 and len(row[2]) == 64, f"bad JXL record: {filename}")
        payload_bytes += size
        items.append(
            {
                "filename": filename,
                "jxl": jxl,
                "jxl_bytes": size,
                "jxl_sha256": row[2],
                "raw_sha256": source[4],
                "width": int(source[5]),
                "height": int(source[6]),
            }
        )
    require(payload_bytes == summary["jxl_payload_bytes"], "payload total changed")
    for name in selected:
        group_root = next(
            item["jxl"].parent for item in items if item["jxl"].parent.name == name
        )
        group_summary_path = group_root / "summary.json"
        group_summary = json.loads(group_summary_path.read_text())
        group_plan = json.loads((group_root / "plan.json").read_text())
        require(
            sha256(group_summary_path) == summary["group_summary_sha256"][name]
            and group_summary["journal_sha256"]
            == sha256(group_root / "completed.jsonl")
            and group_summary["profile"] == summary["profile"]
            and group_plan["groups"] == [name]
            and group_plan["input_manifest_sha256"] == sha256(manifest_path)
            and group_plan["source_tables_sha256"] == {name: table_hashes[name]},
            f"group provenance changed: {name}",
        )
    return summary, items, table_hashes


def run(args: argparse.Namespace) -> None:
    summary, items, table_hashes = load_items(
        args.manifest, args.sources, args.collection, args.allow_subset
    )
    decoder_version = subprocess.run(
        [str(args.decoder), "--version"], capture_output=True, text=True, check=True
    )
    plan = {
        "manifest_sha256": sha256(args.manifest),
        "collection_summary_sha256": sha256(args.collection / "summary.json"),
        "file_list_sha256": sha256(args.collection / "files.tsv"),
        "source_tables_sha256": table_hashes,
        "driver_sha256": sha256(pathlib.Path(__file__)),
        "decoder_sha256": sha256(args.decoder),
        "decoder_version": (decoder_version.stdout + decoder_version.stderr).strip(),
        "images": len(items),
        "pixel_format": "RGB8",
    }
    args.output.mkdir(parents=True, exist_ok=True)
    args.scratch.mkdir(parents=True, exist_ok=True)
    plan_path = args.output / "plan.json"
    if plan_path.exists():
        require(json.loads(plan_path.read_text()) == plan, "resume plan changed")
    else:
        require(not any(args.output.iterdir()), "nonempty output without plan")
        write_json(plan_path, plan)
    result_path = args.output / "verified.jsonl"
    previous = {}
    if result_path.exists():
        for line in result_path.read_text().splitlines():
            record = json.loads(line)
            require(record["filename"] not in previous, "duplicate oracle record")
            previous[record["filename"]] = record
    require(len(previous) <= len(items), "too many oracle records")
    verified = set()
    with result_path.open("a") as stream:
        for index, item in enumerate(items, start=1):
            filename = item["filename"]
            jxl = item["jxl"]
            require(
                jxl.stat().st_size == item["jxl_bytes"]
                and sha256(jxl) == item["jxl_sha256"],
                f"JXL changed: {filename}",
            )
            if filename in previous:
                record = previous[filename]
                require(
                    record["jxl_sha256"] == item["jxl_sha256"]
                    and record["raw_sha256"] == item["raw_sha256"]
                    and record["returncode"] == 0,
                    f"oracle record changed: {filename}",
                )
            else:
                work = args.scratch / pathlib.Path(filename).stem
                work.mkdir()
                decoded_path = work / "decoded.png"
                command = [str(args.decoder), str(jxl), str(decoded_path)]
                process = subprocess.run(command, capture_output=True, check=False)
                if process.returncode != 0:
                    (work / "decoder.log").write_bytes(process.stdout + process.stderr)
                    raise RuntimeError(
                        f"decoder rc={process.returncode}: {filename} ({work})"
                    )
                with Image.open(decoded_path) as decoded:
                    require(
                        decoded.mode == "RGB"
                        and decoded.size == (item["width"], item["height"]),
                        f"decoded layout differs: {filename} ({work})",
                    )
                    actual = hashlib.sha256(decoded.tobytes()).hexdigest()
                require(
                    actual == item["raw_sha256"],
                    f"wrong pixels: {filename} ({work})",
                )
                record = {
                    "filename": filename,
                    "jxl_sha256": item["jxl_sha256"],
                    "raw_sha256": actual,
                    "returncode": process.returncode,
                    "command": command,
                }
                stream.write(json.dumps(record, sort_keys=True) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
                decoded_path.unlink()
                work.rmdir()
            verified.add(filename)
            if index % 126 == 0:
                print(f"verified {index}/{len(items)}", flush=True)
    require(
        len(verified) == len(items) and set(previous).issubset(verified),
        "oracle coverage gap",
    )
    require(sha256(args.decoder) == plan["decoder_sha256"], "decoder changed")
    require(sha256(pathlib.Path(__file__)) == plan["driver_sha256"], "driver changed")
    require(
        sha256(args.manifest) == plan["manifest_sha256"]
        and sha256(args.collection / "summary.json")
        == plan["collection_summary_sha256"]
        and sha256(args.collection / "files.tsv") == plan["file_list_sha256"],
        "merged input changed during decode",
    )
    require(
        all(
            sha256(args.sources / name / "source.tsv") == digest
            for name, digest in table_hashes.items()
        ),
        "source table changed during decode",
    )
    result = {
        "images_exact": len(verified),
        "profile": summary["profile"],
        "decoder_sha256": plan["decoder_sha256"],
        "verified_journal_sha256": sha256(result_path),
        "merged_file_list_sha256": plan["file_list_sha256"],
    }
    write_json(args.output / "summary.json", result)
    print(json.dumps(result, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("decoder", type=pathlib.Path)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("sources", type=pathlib.Path)
    parser.add_argument("collection", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("scratch", type=pathlib.Path)
    parser.add_argument("--allow-subset", action="store_true")
    run(parser.parse_args())
