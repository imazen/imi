"""Run original KADID groups and byte-identical archive packaging, fail-stop.

Generation elapsed times are diagnostics, not a repeated performance benchmark.
Every group includes the pristine reference and all 125 original distortions.
"""
import argparse
import csv
import json
import pathlib
import hashlib
from PIL import Image
from fast_lossless import execute, require, sha


def run(probe, manifest_path, images, output, group_names):
    manifest = json.loads(manifest_path.read_text())
    groups = manifest["groups"]
    if group_names:
        names = group_names.split(",")
        require(len(set(names)) == len(names), "duplicate groups")
        groups = [group for group in groups if group["reference"] in names]
        require(len(groups) == len(names), "missing requested group")
    require(groups and all(len(g["items"]) == 126 for g in groups), "incomplete group")
    output.mkdir()
    identity = {"program_sha256": sha(probe), "driver_sha256": sha(pathlib.Path(__file__)),
                "manifest_sha256": sha(manifest_path), "groups": [g["reference"] for g in groups],
                "effort": 2.5, "threads": 1, "exact_decoded_rgb8": True,
                "scope": "decoded samples and supplied item attributes; not PNG ancillary roundtrip",
                "reference_policy": "pristine pages; previous image animation; no forced inter",
                "keyframe_interval": 0, "image_order": "reference first, then original filename",
                "timing_scope": "generation diagnostics only; repeated performance pending"}
    (output / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
    all_rows, independent, reference = [], [], []
    png_bytes, pixels = 0, 0
    for group in groups:
        name = pathlib.Path(group["reference"]).stem
        table = output / f"{name}.tsv"
        table.write_text("".join("\t".join(str(v) for v in [
            item["filename"], item["reference_filename"], item.get("dmos", ""),
            item.get("variance", ""), item.get("distortion_type", ""),
            item.get("distortion_level", ""), int(item["is_reference"])]) + "\n"
            for item in group["items"]))
        target = output / name
        execute([str(probe), "generate", str(table), str(images), str(target)], output / f"{name}.log")
        sources = [line.split("\t") for line in (target / "source.tsv").read_text().splitlines()]
        require(len(sources) == 126, "source count")
        for item, record in zip(group["items"], sources, strict=True):
            filename, length, encoded_hash, raw_length, raw_hash, width, height = record
            require(filename == item["filename"] and int(length) == item["encoded_bytes"]
                    and int(width) == item["width"] and int(height) == item["height"], "input changed")
            require(int(raw_length) == int(width) * int(height) * 3
                    and len(encoded_hash) == len(raw_hash) == 64, "raw/hash identity")
            require(sha(images / filename) == encoded_hash, "original PNG changed")
            # Independent read-only PNG oracle; never used to generate training data.
            with Image.open(images / filename) as oracle:
                require(oracle.mode == "RGB" and oracle.size == (int(width), int(height)), "oracle layout")
                require(hashlib.sha256(oracle.tobytes()).hexdigest() == raw_hash, "zenpng/Pillow pixels differ")
            png_bytes += int(length)
            pixels += int(width) * int(height)
        with (target / "results.tsv").open() as stream:
            rows = list(csv.DictReader(stream, delimiter="\t"))
        require([row["arm"] for row in rows] == ["independent", "pages", "reference", "previous"], "arm coverage")
        for row in rows:
            row["reference"] = name
            require(int(row["bytes"]) > 0 and int(row["pixels"]) > 0, "invalid measurement")
            if row["arm"] != "independent":
                file = target / (row["arm"] + ".imi")
                require(sha(file) == row["sha256"] and file.stat().st_size == int(row["bytes"]), "output changed")
        all_rows.extend(rows)
        independent.extend(target / "independent" / f"{index:03}.imi" for index in range(126))
        reference.append(target / "reference.imi")
        print(f"completed {name}: {len(all_rows) // 4}/{len(groups)} groups", flush=True)
    archives = {}
    for arm, files, index in [("set-independent", independent, "plain"),
                               ("set-reference", reference, "indexed")]:
        table = output / f"{arm}.txt"
        table.write_text("".join(str(path) + "\n" for path in files))
        log = execute([str(probe), "pack", str(table), str(output / f"{arm}.imijset"), index], output / f"{arm}.log")
        result = json.loads(log)
        require(result["entries"] == len(files) and result["all_entries_identical"], "archive coverage")
        archives[arm] = result
    rows = []
    for arm in ["independent", "pages", "reference", "previous"]:
        matched = [row for row in all_rows if row["arm"] == arm]
        rows.append({"arm": arm, "bytes": sum(int(r["bytes"]) for r in matched),
                     "selected_references": sum(int(r["selected_references"]) for r in matched)})
    for arm, result in archives.items():
        rows.append({"arm": arm, "bytes": result["archive_bytes"],
                     "overhead_bytes": result["archive_bytes"] - result["payload_bytes"],
                     "sequence_index_bytes": result["sequence_index_bytes"]})
    require(sha(probe) == identity["program_sha256"] and sha(manifest_path) == identity["manifest_sha256"], "identity changed")
    summary = {"references": len(groups), "distortions": len(groups) * 125,
               "pixels": pixels, "original_png_bytes": png_bytes,
               "original_complete_score_table_bytes": manifest["score_table_bytes"],
               "full_dataset": len(groups) == 81, "arms": rows, "groups": all_rows,
               "all_pixels_and_item_metadata_exact": True, "archives_byte_identical": True,
               "independent_png_oracle": "Pillow RGB samples, every source image",
               "timing": "not a repeated benchmark; generation diagnostics retained per group"}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "groups"}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("probe", type=pathlib.Path)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("images", type=pathlib.Path)
    parser.add_argument("new_output", type=pathlib.Path)
    parser.add_argument("--groups", help="explicit comma-separated original reference filenames; default all 81")
    args = parser.parse_args()
    run(args.probe.resolve(), args.manifest.resolve(), args.images.resolve(), args.new_output.resolve(), args.groups)
