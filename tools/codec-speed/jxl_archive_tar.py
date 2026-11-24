"""Store verified lossless JXL images and their original item tables in USTAR.

The container is sequential and has no imij-set-style random-access index.
"""

import argparse
import csv
import hashlib
import json
import pathlib
import tarfile


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def digest(path: pathlib.Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def tar_info(name: str, size: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = 0o644
    info.mtime = info.uid = info.gid = 0
    return info


def run(args: argparse.Namespace) -> None:
    manifest = json.loads(args.manifest.read_text())
    require(
        manifest["references"] == 81 and manifest["distortions"] == 10125, "inventory"
    )
    groups = manifest["groups"]
    group_names = [pathlib.Path(group["reference"]).stem for group in groups]
    require(len(set(group_names)) == len(group_names) == 81, "duplicate group")
    expected = [item["filename"] for group in groups for item in group["items"]]
    require(len(expected) == len(set(expected)) == 10206, "duplicate image")
    source = args.collection
    summary = json.loads((source / "summary.json").read_text())
    require(
        summary["source_manifest_sha256"] == digest(args.manifest), "manifest changed"
    )
    merged = "file_list_sha256" in summary
    if merged:
        require(
            summary["profile"] == "max"
            and summary["groups"] == 81
            and summary["images"] == 10206,
            "merged coverage",
        )
        selected = group_names
        file_list = source / "files.tsv"
        require(summary["file_list_sha256"] == digest(file_list), "file list changed")
        records = [line.split("\t") for line in file_list.read_text().splitlines()]
    else:
        plan = json.loads((source / "plan.json").read_text())
        require(plan["profile"] == summary["profile"], "profile changed")
        require(plan["input_manifest_sha256"] == digest(args.manifest), "plan changed")
        selected = [name for name in group_names if name in plan["groups"]]
        require(
            len(selected) == len(plan["groups"]) == summary["groups"], "group coverage"
        )
        require(args.allow_subset or len(selected) == 81, "incomplete archive")
        journal = source / "completed.jsonl"
        require(summary["journal_sha256"] == digest(journal), "journal changed")
        records = [json.loads(line) for line in journal.read_text().splitlines()]
    selected_items = [
        item
        for group in groups
        if pathlib.Path(group["reference"]).stem in selected
        for item in group["items"]
    ]
    require(len(records) == len(selected_items) == summary["images"], "image coverage")
    metadata = []
    item_rows = []
    for name in selected:
        path = args.item_tables / f"{name}.tsv"
        require(path.is_file(), f"missing item table: {name}")
        metadata.append((f"metadata/{name}.tsv", path, digest(path)))
        with path.open(newline="") as stream:
            rows = list(csv.reader(stream, delimiter="\t"))
        require(len(rows) == 126 and all(len(row) == 7 for row in rows), "item table")
        item_rows.extend(rows)
    images = []
    for item, record, item_row in zip(selected_items, records, item_rows, strict=True):
        filename = item["filename"]
        require(
            pathlib.Path(filename).name == filename
            and filename.endswith(".png")
            and item_row[0] == filename,
            "name",
        )
        if merged:
            require(
                len(record) == 14
                and record[3] == filename
                and record[3:10] == item_row
                and int(record[12]) == item["width"]
                and int(record[13]) == item["height"],
                f"merged item changed: {filename}",
            )
            jxl = pathlib.Path(record[0])
            length, expected_hash = int(record[1]), record[2]
        else:
            require(
                filename == record["filename"]
                and item["encoded_bytes"] == record["png_bytes"]
                and item["width"] * item["height"] == record["pixels"]
                and record["returncode"] == 0,
                f"source changed: {filename}",
            )
            jxl = source / (pathlib.Path(filename).stem + ".jxl")
            length, expected_hash = record["jxl_bytes"], record["jxl_sha256"]
        require(
            jxl.name == pathlib.Path(filename).stem + ".jxl"
            and jxl.stat().st_size == length
            and digest(jxl) == expected_hash,
            f"JXL changed: {filename}",
        )
        images.append((f"images/{jxl.name}", jxl, expected_hash))
    require(
        sum(path.stat().st_size for _, path, _ in images)
        == summary["jxl_payload_bytes"],
        "payload total changed",
    )
    require(
        not args.archive.exists() and not args.report.exists(),
        "archive or report already exists",
    )
    partial = args.archive.with_name(args.archive.name + ".partial")
    require(not partial.exists(), "partial archive needs inspection")
    all_entries = metadata + images
    with tarfile.open(partial, "w", format=tarfile.USTAR_FORMAT) as archive:
        for name, path, _ in all_entries:
            with path.open("rb") as stream:
                archive.addfile(tar_info(name, path.stat().st_size), stream)
    partial.replace(args.archive)
    with tarfile.open(args.archive, "r:") as archive:
        members = archive.getmembers()
        require(
            len(members) == len(all_entries)
            and [member.name for member in members]
            == [name for name, _, _ in all_entries],
            "tar member inventory changed",
        )
        for member, (_, _, expected_hash) in zip(members, all_entries, strict=True):
            content = archive.extractfile(member)
            require(content is not None, f"missing tar member: {member.name}")
            value = hashlib.sha256()
            for chunk in iter(lambda: content.read(1024 * 1024), b""):
                value.update(chunk)
            require(
                value.hexdigest() == expected_hash, f"tar member changed: {member.name}"
            )
    report = {
        "profile": summary["profile"],
        "images": len(images),
        "metadata_tables": len(metadata),
        "jxl_payload_bytes": summary["jxl_payload_bytes"],
        "metadata_table_bytes": sum(path.stat().st_size for _, path, _ in metadata),
        "ustar_archive_bytes": args.archive.stat().st_size,
        "ustar_archive_sha256": digest(args.archive),
        "collection_summary_sha256": digest(source / "summary.json"),
        "source_manifest_sha256": digest(args.manifest),
        "container": "USTAR, sequential; no random-access index",
        "all_member_hashes_match": True,
    }
    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("item_tables", type=pathlib.Path)
    parser.add_argument("collection", type=pathlib.Path)
    parser.add_argument("archive", type=pathlib.Path)
    parser.add_argument("report", type=pathlib.Path)
    parser.add_argument("--allow-subset", action="store_true")
    run(parser.parse_args())
