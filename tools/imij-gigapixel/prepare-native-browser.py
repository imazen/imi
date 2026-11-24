#!/usr/bin/env python3
"""Stage one or more native pyramid representations for the Explorer.

The catalog supplies publication URLs, opening ranges and control geometry.
The reader validates those bytes and geometry against the native directory.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import sys


def stage(packed, output, key, part_size, publication=False):
    manifest = json.loads((packed / "pyramid.json").read_text())
    file = packed / "pyramid.imi"
    length = file.stat().st_size
    if manifest["encodedBytes"] != length:
        raise ValueError("native pyramid length")
    url = f"/image/{key}.imi"
    filename = f"{key}.imi"
    parts_name = f"{key}-parts"
    parts_base = f"/image/{parts_name}/"
    if publication:
        content_id = manifest["contentId"]
        if not re.fullmatch(r"[0-9a-f]{32}", content_id):
            raise ValueError("native pyramid content ID")
        relative = Path("d") / content_id
        url, filename = f"/{relative}/pyramid.imi", relative / "pyramid.imi"
        parts_name, parts_base = relative / "p", f"/{relative}/p"
        (output / relative).mkdir(parents=True)
    if part_size:
        part_dir = output / parts_name
        part_dir.mkdir()
        count = 0
        with file.open("rb") as inp:
            while block := inp.read(part_size):
                with (part_dir / f"{count:06}.bin").open("xb") as out:
                    out.write(block)
                count += 1
        location = {"parts": {"base": parts_base, "size": part_size, "count": count}}
    else:
        shutil.copyfile(file, output / filename)
        location = {"url": url}
    reps = {}
    levels = []
    for view in manifest["views"]:
        divisor = view["divisor"]
        level = {1: "full", 2: "half", 4: "quarter"}.get(divisor, f"level-{divisor}")
        levels.append({"key": level, "divisor": divisor})
        reps[level] = {
            **location, "kind": "native", "contentId": manifest["contentId"],
            "encodedBytes": length, "viewId": view["id"], "width": view["width"],
            "height": view["height"], "divisor": divisor, "qualityTarget": manifest["qualityTarget"],
            "multipart": False, "wireVersion": 4,
        }
        if manifest.get("opening"):
            reps[level]["opening"] = manifest["opening"]
    return levels, reps


def replace():
    parser = argparse.ArgumentParser(description="Replace a catalog's scene/quality with one native pyramid.")
    parser.add_argument("catalog", type=Path)
    parser.add_argument("scene")
    parser.add_argument("quality")
    parser.add_argument("packed", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--part-size", type=int, default=32 * 1024 * 1024)
    args = parser.parse_args(sys.argv[2:])
    if not 1 <= args.part_size <= 512 * 1024 * 1024:
        parser.error("publication part size must be between 1 byte and 512 MiB")
    catalog = json.loads(args.catalog.read_text())
    scene = next(row for row in catalog["datasets"] if row["id"] == args.scene)
    previous = scene["representations"][args.quality]
    manifest = json.loads((args.packed / "pyramid.json").read_text())
    views = {v["divisor"]: v for v in manifest["views"]}
    for level in scene["levels"]:
        old, view = previous[level["key"]], views[level["divisor"]]
        if (old["width"], old["height"], old.get("qualityTarget")) != (
            view["width"], view["height"], manifest["qualityTarget"]
        ):
            raise ValueError("native replacement disagrees with catalog geometry or quality")
    args.output.mkdir(parents=True)
    levels, reps = stage(args.packed, args.output, args.quality, args.part_size, publication=True)
    by_divisor = {row["divisor"]: reps[row["key"]] for row in levels}
    scene["representations"][args.quality] = {
        row["key"]: by_divisor[row["divisor"]] for row in scene["levels"]
    }
    with (args.output / "image.json").open("x") as file:
        json.dump(catalog, file)
        file.write("\n")


def main():
    if sys.argv[1:2] == ["replace"]:
        replace()
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lossless", type=Path)
    parser.add_argument("lossy", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--part-size", type=int, default=0)
    parser.add_argument("--id", default="v4-pattern")
    parser.add_argument("--title", default="Native V4 pyramid")
    parser.add_argument("--source-url", default="https://github.com/imazen/zqi")
    parser.add_argument("--credit", default="Local source")
    parser.add_argument("--source", type=Path, help="optional RGB8 pixel oracle for the browser gate")
    args = parser.parse_args()
    if not 0 <= args.part_size <= 512 * 1024 * 1024:
        parser.error("--part-size must be zero or at most 512 MiB")
    args.output.mkdir(parents=True)
    levels, lossless = stage(args.lossless, args.output, "lossless", args.part_size)
    lossy_levels, lossy = stage(args.lossy, args.output, "lossy", args.part_size)
    if levels != lossy_levels or any(
        (lossless[key]["width"], lossless[key]["height"]) != (lossy[key]["width"], lossy[key]["height"])
        for key in lossless
    ):
        raise ValueError("native pyramid representations disagree on geometry")
    scene = {
        "id": args.id, "title": args.title,
        "sourceUrl": args.source_url, "credit": args.credit,
        "width": lossless["full"]["width"], "height": lossless["full"]["height"],
        "levels": levels, "representations": {"lossless": lossless, "lossy": lossy},
    }
    for name in ("image.json", "gallery.json"):
        with (args.output / name).open("x") as out:
            json.dump({"datasets": [scene]}, out)
            out.write("\n")
    if args.source:
        shutil.copyfile(args.source, args.output / "source.rgb")


if __name__ == "__main__":
    main()
