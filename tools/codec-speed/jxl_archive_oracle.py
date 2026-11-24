"""Check every collected JXL against the original KADID RGB8 raster hash."""

import argparse
import csv
import hashlib
import json
import os
import pathlib
import subprocess

from PIL import Image


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: pathlib.Path, value: object) -> None:
    temporary = path.with_name(path.name + ".partial")
    require(not temporary.exists(), f"partial record needs inspection: {temporary}")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def run(args: argparse.Namespace) -> None:
    size_plan = json.loads((args.collection / "plan.json").read_text())
    size_summary = json.loads((args.collection / "summary.json").read_text())
    journal = args.collection / "completed.jsonl"
    require(sha256(journal) == size_summary["journal_sha256"], "size journal changed")
    source_records: dict[str, tuple[str, str, str]] = {}
    for group in size_plan["groups"]:
        table = args.sources / group / "source.tsv"
        require(
            sha256(table) == size_plan["source_tables_sha256"][group],
            "original source table changed",
        )
        with table.open(newline="") as stream:
            for row in csv.reader(stream, delimiter="\t"):
                require(
                    len(row) == 7 and row[0] not in source_records, "bad source table"
                )
                source_records[row[0]] = (row[4], row[5], row[6])
    encoded = [json.loads(line) for line in journal.read_text().splitlines()]
    require(
        len(encoded)
        == size_plan["images"]
        == size_summary["images"]
        == len(source_records),
        "incomplete size collection",
    )
    require(
        len({row["filename"] for row in encoded}) == len(encoded), "duplicate image"
    )
    decoder_version = subprocess.run(
        [str(args.decoder), "--version"], capture_output=True, text=True, check=True
    )
    plan = {
        "collection_plan_sha256": sha256(args.collection / "plan.json"),
        "collection_summary_sha256": sha256(args.collection / "summary.json"),
        "journal_sha256": sha256(journal),
        "driver_sha256": sha256(pathlib.Path(__file__)),
        "decoder_sha256": sha256(args.decoder),
        "decoder_version": (decoder_version.stdout + decoder_version.stderr).strip(),
        "source_tables_sha256": size_plan["source_tables_sha256"],
        "images": len(encoded),
        "pixel_format": "RGB8",
    }
    args.output.mkdir(parents=True, exist_ok=True)
    args.scratch.mkdir(parents=True, exist_ok=True)
    if (args.output / "plan.json").exists():
        require(
            json.loads((args.output / "plan.json").read_text()) == plan,
            "resume plan changed",
        )
    else:
        require(not any(args.output.iterdir()), "nonempty output without plan")
        write_json(args.output / "plan.json", plan)
    results = args.output / "verified.jsonl"
    previous: dict[str, dict] = {}
    if results.exists():
        for line in results.read_text().splitlines():
            row = json.loads(line)
            require(row["filename"] not in previous, "duplicate oracle record")
            previous[row["filename"]] = row
    require(len(previous) <= len(encoded), "too many oracle records")
    checked = set()
    with results.open("a") as stream:
        for index, item in enumerate(encoded, start=1):
            filename = item["filename"]
            require(filename in source_records, "unexpected encoded image")
            expected_raw, width, height = source_records[filename]
            jxl = args.collection / (pathlib.Path(filename).stem + ".jxl")
            require(
                jxl.stat().st_size == item["jxl_bytes"]
                and sha256(jxl) == item["jxl_sha256"],
                f"JXL changed: {filename}",
            )
            if filename in previous:
                old = previous[filename]
                require(
                    old["jxl_sha256"] == item["jxl_sha256"]
                    and old["raw_sha256"] == expected_raw
                    and old["returncode"] == 0,
                    "oracle record changed",
                )
            else:
                work = args.scratch / pathlib.Path(filename).stem
                work.mkdir()
                image_path = work / "decoded.png"
                command = [str(args.decoder), str(jxl), str(image_path)]
                result = subprocess.run(command, capture_output=True, check=False)
                if result.returncode != 0:
                    (work / "decoder.log").write_bytes(result.stdout + result.stderr)
                    raise RuntimeError(
                        f"decoder rc={result.returncode}: {filename} ({work})"
                    )
                with Image.open(image_path) as decoded:
                    require(
                        decoded.mode == "RGB"
                        and decoded.size == (int(width), int(height)),
                        f"decoded layout differs: {filename} ({work})",
                    )
                    actual_raw = hashlib.sha256(decoded.tobytes()).hexdigest()
                require(
                    actual_raw == expected_raw, f"wrong pixels: {filename} ({work})"
                )
                record = {
                    "filename": filename,
                    "jxl_sha256": item["jxl_sha256"],
                    "raw_sha256": actual_raw,
                    "returncode": result.returncode,
                    "command": command,
                }
                stream.write(json.dumps(record, sort_keys=True) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
                image_path.unlink()
                work.rmdir()
            checked.add(filename)
            if index % 126 == 0:
                print(f"verified {index}/{len(encoded)}", flush=True)
    require(
        checked == set(source_records) and set(previous).issubset(checked),
        "oracle coverage gap",
    )
    require(sha256(args.decoder) == plan["decoder_sha256"], "decoder changed")
    require(sha256(pathlib.Path(__file__)) == plan["driver_sha256"], "driver changed")
    summary = {
        "images_exact": len(checked),
        "profile": size_plan["profile"],
        "decoder_sha256": plan["decoder_sha256"],
        "verified_journal_sha256": sha256(results),
    }
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("decoder", type=pathlib.Path)
    parser.add_argument("sources", type=pathlib.Path)
    parser.add_argument("collection", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("scratch", type=pathlib.Path)
    run(parser.parse_args())
