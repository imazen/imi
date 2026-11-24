#!/usr/bin/env python3
"""Download four adjacent original NASA tiles; import with zenpng, encode one IMI.

Run through the normal host/resource wrapper. Outputs must be a fresh directory.
No image is rescaled: this is the western half of the 500 m July 2004 mosaic.
"""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from range_download import download

p = argparse.ArgumentParser()
p.add_argument("program", type=Path)
p.add_argument("output", type=Path)
a = p.parse_args()
a.program = a.program.resolve(strict=True)
a.output.mkdir(parents=True, exist_ok=False)
records = []


for tile in ("A1", "B1", "A2", "B2"):
    name = f"world.topo.bathy.200407.3x21600x21600.{tile}.png"
    url = f"https://eoimages.gsfc.nasa.gov/images/imagerecords/73000/73751/{name}"
    source = a.output / name
    print(f"download {tile}: {url}", flush=True)
    source_hash = download(url, source)
    raw = a.output / f"{tile}.rgb"
    subprocess.run([str(a.program), "png", str(source), str(raw)], check=True)
    raw_hash = hashlib.sha256()
    with raw.open("rb") as f:
        while chunk := f.read(1 << 20):
            raw_hash.update(chunk)
    records.append({"tile": tile, "url": url, "source_sha256": source_hash,
                    "raw_sha256": raw_hash.hexdigest(), "raw_bytes": raw.stat().st_size})
    (a.output / "sources.json").write_text(json.dumps(records, indent=2) + "\n")
subprocess.run([str(a.program), "grid", "21600", "512", "256",
                *[str(a.output / f"{tile}.rgb") for tile in ("A1", "B1", "A2", "B2")],
                str(a.output / "blue-marble.imi")], check=True)
manifest = {
    "title": "Blue Marble · Western Hemisphere", "width": 43200, "height": 43200,
    "pixels": 43200 * 43200, "file": "blue-marble.imi", "groupWidth": 512,
    "groupHeight": 256, "maxSourcePixels": 16_777_216, "maxZoomOut": 4,
    "initial": {"x": 14900, "y": 11200, "scale": 1},
    "credit": "NASA Earth Observatory / Reto Stöckli",
    "sourceUrl": "https://visibleearth.nasa.gov/images/73751/july-blue-marble-next-generation-w-topography-and-bathymetry",
    "description": "Four adjacent original 500 m NASA tiles, July 2004. No upscaling. Lossless RGB8 IMI; one resolution, no image pyramid.",
    "sources": records,
}
(a.output / "image.json").write_text(json.dumps(manifest, indent=2) + "\n")
