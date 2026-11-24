#!/usr/bin/env python3
"""Pack normalized full-reference IQA stills and their stored scores."""
import argparse
import csv
from io import BytesIO
import json
import math
import mimetypes
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory

import xxhash
from PIL import Image, ImageCms, ImageOps

PROFILE = "imij.iqa/v1"
PAGE_SIZE = 64


def tsv(path):
    with path.open(encoding="utf-8", newline="") as handle:
        yield from csv.DictReader(handle, delimiter="\t")


def number(text):
    value = float(text)
    if not math.isfinite(value):
        raise ValueError(f"nonfinite score or scale {text!r}")
    return value


def score(row, kind, source):
    name = row["score_kind" if kind == "human" else "model"]
    rightward = row.get("higher_is_better")
    if not name or rightward not in ("", None, "0", "1"):
        raise ValueError(f"invalid score metadata for {row['item']}")
    result = dict(kind=kind, name=name, value=number(row["score"]), source=source)
    for field, renamed in [("scale_min", "min"), ("scale_max", "max"), ("std", "std")]:
        if row.get(field):
            result[renamed] = number(row[field])
    if rightward:
        result["higherIsBetter"] = rightward == "1"
    if row.get("version"):
        result["version"] = row["version"]
    return result


def blob(path, value, limit):
    data = json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()
    if len(data) > limit:
        raise ValueError(f"catalog object exceeds {limit} bytes")
    path.write_bytes(data)


def identity(path):
    digest = xxhash.xxh3_128()
    with path.open("rb") as source:
        for part in iter(lambda: source.read(1 << 20), b""):
            digest.update(part)
    return {"id": 1, "bytes": digest.intdigest().to_bytes(16, "little").hex()}


def thumbnail(source, target):
    with Image.open(source) as original:
        width, height = original.size
        profile = original.info.get("icc_profile")
        image = ImageOps.exif_transpose(original)
        image.thumbnail((256, 256))
        if profile:
            if image.mode == "RGBA":
                image = image.convert("RGB")
            image = ImageCms.profileToProfile(
                image, ImageCms.ImageCmsProfile(BytesIO(profile)),
                ImageCms.createProfile("sRGB"), outputMode="RGB",
            )
        elif image.mode != "RGB":
            image = image.convert("RGB")
        image.save(target, format="JPEG", quality=85)
        return (width, height), image.size


def pack(args, scratch):
    content = (args.dataset / "content").resolve()
    with (args.dataset / "MANIFEST.json").open(encoding="utf-8") as handle:
        metadata = json.load(handle)
    labeled = (row for row in tsv(args.dataset / "labels.tsv") if row.get("ref"))
    rows = []
    for row in labeled:
        rows.append(row)
        if args.limit and len(rows) == args.limit:
            break
    if not rows:
        raise ValueError("labels.tsv contains no full-reference rows")
    page_count = (len(rows) + PAGE_SIZE - 1) // PAGE_SIZE
    if page_count > 1_000_000:
        raise ValueError("pair page count exceeds profile limit")
    model_path = args.model_scores or args.dataset / "model_scores.tsv"
    selected = {(row["item"], row["ref"]) for row in rows}
    model_scores = {}
    if model_path.is_file():
        for row in tsv(model_path):
            pair = row["item"], row.get("ref", "")
            if pair in selected:
                model_scores.setdefault(pair, []).append(score(row, "metric", model_path.name))

    files = []
    assets = {}

    def asset(name):
        if name in assets:
            return assets[name]
        source = (content / name).resolve()
        if not source.is_relative_to(content) or not source.is_file():
            raise ValueError(f"invalid dataset content path {name!r}")
        key = len(files) + page_count + 1
        preview = scratch / f"{key}.jpg"
        (width, height), (thumb_w, thumb_h) = thumbnail(source, preview)
        media = mimetypes.guess_type(source.name)[0] or "application/octet-stream"
        files.extend([
            (key, name, media, "", width, height, source),
            (key + 1, f"Thumbnail · {name}", "image/jpeg", "", thumb_w, thumb_h, preview),
        ])
        assets[name] = {"key": str(key), "thumbnail": str(key + 1), "name": name,
                        "mediaType": media, "digest": identity(source)}
        return assets[name]

    pages = []
    for page_start in range(0, len(rows), PAGE_SIZE):
        pairs = []
        for i, row in enumerate(rows[page_start:page_start + PAGE_SIZE], page_start):
            pair = row["item"], row["ref"]
            scores = [score(row, "human", "labels.tsv")] if row.get("score") else []
            scores.extend(model_scores.get(pair, []))
            result = {
                "id": str(i + 1),
                "reference": asset(row["ref"]),
                "distorted": asset(row["item"]),
                "scores": scores,
            }
            for source, dest in [("source_id", "source"), ("distortion", "distortion"),
                                 ("level", "level"), ("split", "split")]:
                if row.get(source):
                    result[dest] = row[source]
            pairs.append(result)
        page_index = len(pages)
        path = scratch / f"pairs-{page_index}.json"
        blob(path, {"schema": PROFILE + "/page", "index": page_index, "pairs": pairs}, 1 << 20)
        pages.append((page_index + 1, f"Pair page {page_index + 1}", "application/json", "",
                      0, 0, path))

    root = scratch / "catalog.json"
    overview = {key: metadata[key] for key in ("citation", "doi_or_arxiv", "license", "version")
                if isinstance(metadata.get(key), str) and metadata[key]}
    dataset_id = str(metadata.get("dataset_id") or args.dataset.name)
    blob(root, {
        "schema": PROFILE, "dataset": {"id": dataset_id, "name": metadata.get("name") or dataset_id,
                                        "metadata": overview},
        "pairs": {"firstPage": "1", "pageCount": page_count, "pageSize": PAGE_SIZE,
                  "count": len(rows)},
    }, 1 << 16)
    plan = scratch / "entries.tsv"
    with plan.open("w", encoding="utf-8") as handle:
        for row in [(0, "IQA catalog", "application/json", PROFILE, 0, 0, root), *pages, *files]:
            fields = tuple(map(str, row))
            if any("\t" in value or "\n" in value or "\r" in value for value in fields):
                raise ValueError("archive entry plan requires single-line fields")
            print(*fields, sep="\t", file=handle)
    subprocess.run([args.packer, plan, args.output], check=True)
    print(f"{len(rows)} pairs, {len(assets)} exact source assets, {page_count} catalog pages")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dataset", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--packer", type=Path, required=True)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--model-scores", type=Path)
    args = parser.parse_args()
    if args.limit is not None and args.limit <= 0:
        parser.error("--limit must be positive")
    if args.model_scores is not None and not args.model_scores.is_file():
        parser.error("--model-scores must name a readable file")
    with TemporaryDirectory(prefix="iqa-pack-", dir=args.output.resolve().parent) as tmp:
        pack(args, Path(tmp))


if __name__ == "__main__":
    main()
