#!/usr/bin/env python3
"""Pack one frozen IQA image group without changing any encoded image bytes."""

import argparse
import hashlib
import io
import json
import subprocess
import tarfile
import zipfile
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import zarr


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_digest(path: Path) -> str:
    sha = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            sha.update(block)
    return sha.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_sources(archive_root: Path, input_dir: Path, manifest: Path) -> list[tuple[str, bytes]]:
    records = {row["artifact"]: row for row in json.loads(manifest.read_text())}
    paths = sorted(input_dir.glob("*.imi"))
    require(len(paths) == 126, "I01 must contain all 126 encoded images")
    require([path.name for path in paths] == [f"{n:03}.imi" for n in range(126)], "image ordinals")
    result = []
    for path in paths:
        name = path.relative_to(archive_root).as_posix()
        require(name in records, f"missing sealed source: {name}")
        data = path.read_bytes()
        row = records[name]
        require(len(data) == row["bytes"] and digest(data) == row["sha256"], f"source drift: {name}")
        result.append((path.name, data))
    return result


def check_entries(actual: list[tuple[str, bytes]], expected: list[tuple[str, bytes]], label: str) -> None:
    require(len(actual) == len(expected), f"{label}: entry count")
    for (name, data), (want_name, want_data) in zip(actual, expected):
        require(name == want_name and data == want_data, f"{label}: {want_name} changed")


def write_zip(path: Path, entries: list[tuple[str, bytes]]) -> None:
    with zipfile.ZipFile(path, "w", allowZip64=True) as archive:
        for name, data in entries:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED
            archive.writestr(info, data)
    with zipfile.ZipFile(path) as archive:
        require(archive.namelist() == [name for name, _ in entries], "ZIP entry names")
        check_entries([(name, archive.read(name)) for name, _ in entries], entries, "ZIP")


def write_tar(path: Path, entries: list[tuple[str, bytes]]) -> None:
    with tarfile.open(path, "w", format=tarfile.PAX_FORMAT) as archive:
        for name, data in entries:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mtime = 0
            archive.addfile(info, io.BytesIO(data))
    with tarfile.open(path) as archive:
        members = archive.getmembers()
        check_entries([(member.name, archive.extractfile(member).read()) for member in members], entries, "tar")


def write_parquet(path: Path, entries: list[tuple[str, bytes]]) -> None:
    table = pa.table({
        "key": pa.array([name for name, _ in entries], type=pa.string()),
        "payload": pa.array([data for _, data in entries], type=pa.binary()),
    })
    pq.write_table(table, path, compression="NONE", use_dictionary=False, row_group_size=16)
    decoded = pq.read_table(path, columns=["key", "payload"])
    check_entries(list(zip(decoded["key"].to_pylist(), decoded["payload"].to_pylist())), entries, "Parquet")


def write_zarr(path: Path, entries: list[tuple[str, bytes]]) -> None:
    offsets = []
    position = 0
    for name, data in entries:
        offsets.append({"key": name, "offset": position, "length": len(data)})
        position += len(data)
    array = zarr.create_array(
        store=str(path), shape=(position,), dtype="u1", chunks=(64 * 1024,),
        shards=(1024 * 1024,), compressors=[], zarr_format=3,
        attributes={"entries": offsets},
    )
    encoded = b"".join(data for _, data in entries)
    array[:] = np.frombuffer(encoded, dtype=np.uint8)
    reopened = zarr.open_array(store=str(path), mode="r")
    require(reopened.attrs["entries"] == offsets, "Zarr offset index")
    check_entries(
        [(row["key"], reopened[row["offset"]:row["offset"] + row["length"]].tobytes())
         for row in reopened.attrs["entries"]],
        entries,
        "Zarr",
    )


def describe(path: Path) -> dict:
    if path.is_file():
        return {"bytes": path.stat().st_size, "files": 1, "sha256": file_digest(path)}
    files = sorted(child for child in path.rglob("*") if child.is_file())
    require(files and not any(child.is_symlink() for child in path.rglob("*")), "Zarr contents")
    listing = "".join(
        f"{child.relative_to(path).as_posix()}\t{child.stat().st_size}\t{file_digest(child)}\n"
        for child in files
    )
    return {"bytes": sum(child.stat().st_size for child in files), "files": len(files),
            "content_manifest_sha256": digest(listing.encode()), "content_manifest": listing}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive_root", type=Path)
    parser.add_argument("input_dir", type=Path)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("packer", type=Path)
    parser.add_argument("packer_sha256")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    require(file_digest(args.packer) == args.packer_sha256, "IMIJ packer binary changed")
    entries = load_sources(args.archive_root, args.input_dir, args.manifest)
    args.output.mkdir(parents=True, exist_ok=False)
    source_bytes = sum(len(data) for _, data in entries)
    table = args.output / "source-paths.txt"
    table.write_text("".join(f"{args.input_dir / name}\n" for name, _ in entries))

    zip_path = args.output / "images.zip"
    tar_path = args.output / "images.tar"
    parquet_path = args.output / "images.parquet"
    zarr_path = args.output / "images.zarr"
    set_path = args.output / "images.imijset"
    write_zip(zip_path, entries)
    write_tar(tar_path, entries)
    write_parquet(parquet_path, entries)
    write_zarr(zarr_path, entries)
    pack = subprocess.run(
        [str(args.packer), "pack", str(table), str(set_path), "plain"],
        check=True, capture_output=True, text=True,
    )
    (args.output / "imij-set-pack.stdout").write_text(pack.stdout)
    (args.output / "imij-set-pack.stderr").write_text(pack.stderr)
    set_result = json.loads(pack.stdout)
    require(set_result["entries"] == len(entries) and set_result["payload_bytes"] == source_bytes
            and set_result["all_entries_identical"] is True, "IMIJ-set pack/readback")

    outputs = {
        "zip_stored": describe(zip_path), "tar_pax": describe(tar_path),
        "parquet_binary": describe(parquet_path), "zarr_v3_uint8_shards": describe(zarr_path),
        "imij_set": describe(set_path),
    }
    for value in outputs.values():
        value["overhead_bytes"] = value["bytes"] - source_bytes
    report = {
        "scope": "I01 126 frozen independent IMIJ images; container overhead only, no HTTP or pixel timing",
        "source_bytes": source_bytes,
        "entries": len(entries),
        "source_manifest_sha256": file_digest(args.manifest),
        "packer_sha256": args.packer_sha256,
        "python_libraries": {"numpy": np.__version__, "pyarrow": pa.__version__, "zarr": zarr.__version__},
        "layout": {"zip": "stored ZIP, ZIP64 permitted", "tar": "uncompressed tar, PAX permitted", "parquet": "uncompressed binary payload, 16 rows per group", "zarr": "uint8 concatenation, 64 KiB chunks, 1 MiB shards, JSON offset index"},
        "outputs": outputs,
    }
    (args.output / "results.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"entries": len(entries), "source_bytes": source_bytes,
                      "sizes": {key: value["bytes"] for key, value in outputs.items()}}, sort_keys=True))


if __name__ == "__main__":
    main()
