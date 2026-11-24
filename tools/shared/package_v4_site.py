#!/usr/bin/env python3
"""Package current source with explicitly supplied, qualified codec artifacts.

The base supplies retained immutable bundles, image catalog and Atlas examples.
Nothing is deployed. All outputs must be fresh; existing artifacts stay intact.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory

from refresh_site_docs import atlas_entry, preserve_bundles


def atlas_data(path):
    text = path.read_text()
    prefix = "window.ATLAS="
    if not text.startswith(prefix):
        raise ValueError(f"not an Atlas bundle: {path}")
    return json.loads(text[len(prefix):].strip().removesuffix(";"))


def package(base, dist, atlas, output, codec_source, catalog=None):
    root = Path(__file__).resolve().parents[2]
    web = root / "tools/imij-wasm/web"
    if output.exists():
        raise ValueError(f"choose a fresh output directory: {output}")
    for name in ("index.html", "image/image.json", "codestream-atlas/data.js"):
        if not (base / name).is_file():
            raise ValueError(f"base is missing {name}")
    for name in ("pkg-simd", "pkg-relaxed"):
        for asset in ("imij.js", "imij_bg.wasm"):
            if not (dist / name / asset).is_file():
                raise ValueError(f"qualified decoder is missing {name}/{asset}")
    for name in ("data.js", "imij-wasm.wasm"):
        if not (atlas / name).is_file():
            raise ValueError(f"qualified Atlas is missing {name}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix=".imi-package-", dir=output.parent) as directory:
        work = Path(directory)
        candidate = work / "site"
        assemble(root, web, base, dist, atlas, candidate, work / "inputs", codec_source, catalog)
        if output.exists():
            raise ValueError(f"output appeared while packaging: {output}")
        candidate.rename(output)


def assemble(root, web, base, dist, atlas, output, inputs, codec_source, catalog):
    inputs.mkdir()
    for name in ("archive", "gigapixel", "src", "site"):
        shutil.copytree(web / name, inputs / name)
    subprocess.run(["python3", str(web / "site/build-spec-page.py"),
                    str(inputs / "site/spec.html")], check=True)
    # Feed the publisher its origin-relative catalog form, just as the existing
    # documentation refresh does. This also retains split-object validation.
    manifest = inputs / "image.json"
    manifest.write_text(re.sub(r'"https?://[^/\"]+/(d|p)/', r'"/\1/',
                              (catalog or base / "image/image.json").read_text()))
    subprocess.run(["node", str(web / "scripts/package-gigapixel.mjs"),
                    str(inputs), str(dist / "pkg-simd"),
                    str(manifest), str(output),
                    "https://gigapixels.imazen.org"], check=True)
    preserve_bundles(base, output)
    target = output / "codestream-atlas"
    target.mkdir()
    source = root / "tools/codestream-atlas"
    for name in ("index.html", "wasi-worker.js", "v4-grid.js"):
        shutil.copyfile(source / name, target / name)
    shutil.copyfile(atlas / "imij-wasm.wasm", target / "imij-wasm.wasm")
    base_atlas = base / "codestream-atlas"
    retained = atlas_data(base_atlas / "data.js")
    current_files = []
    if atlas == base_atlas:
        shutil.copyfile(base_atlas / "data.js", target / "data.js")
    else:
        current = atlas_data(atlas / "data.js")
        retained["spec"].update(current["spec"])
        current_files = current["files"]
        if current.get("reviewGallery"):
            current_files = [item for item in current_files if item.get("review")]
            if not current_files:
                raise ValueError("review gallery has no review examples")
            retained["reviewGallery"] = True
        for item in current_files:
            if not item["name"].startswith("V4 · "):
                item["name"] = "V4 · " + item["name"]
            retained["files"].append(item)
        sources = current.get("sources", []) + retained.get("sources", [])
        retained["sources"] = []
        source_names = set()
        for item in sources:
            if item["name"] not in source_names:
                source_names.add(item["name"])
                retained["sources"].append(item)
        (target / "data.js").write_text("window.ATLAS=" + json.dumps(retained) + ";\n")
    index = target / "index.html"
    index.write_text(atlas_entry(index.read_text(), "https://imi.imazen.org"))
    with (output / "_headers").open("a") as headers:
        headers.write("/spec/*\n  Cache-Control: public, max-age=300\n/imi/*\n  Cache-Control: no-cache\n/codestream-atlas/*\n  Cache-Control: no-cache\n")
    (output / "spec").mkdir()
    for src, dst in (("spec.html", "index.html"), ("style.css", "style.css")):
        shutil.copyfile(output / "imi" / src, output / "spec" / dst)
    (output / "BUILD.json").write_text(json.dumps({
        "codec_source": codec_source, "retained_atlas_files": len(retained["files"]) - len(current_files),
        "v4_atlas_files": len(current_files), "deployed": False,
    }, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("base", "dist", "atlas", "output"):
        parser.add_argument(name, type=lambda p: Path(p).resolve())
    parser.add_argument("codec_source", help="revision used for the qualified codec binaries")
    parser.add_argument("--catalog", type=Path, help="replacement image catalog with published data URLs")
    args = parser.parse_args()
    package(args.base, args.dist, args.atlas, args.output, args.codec_source, args.catalog)
