#!/usr/bin/env python3
"""Repackage current docs with a complete stage's existing decoder/Atlas binaries."""
from pathlib import Path
import re
import shutil
import subprocess
import sys


def preserve_bundles(source, output):
    source, output = Path(source), Path(output)
    for path in (source / "bundles").rglob("*"):
        if path.is_file():
            target = output / path.relative_to(source)
            if target.exists():
                if target.read_bytes() != path.read_bytes():
                    raise ValueError(f"immutable bundle collision: {target}")
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, target)


ATLAS_HEAD = re.compile(r'<link rel="canonical"[^>]*>\n?|<script src="[^"]*site-origin\.js"></script>\n?')


def atlas_entry(text, page_origin):
    """Give a retained Atlas page the canonical URL and the shared origin script."""
    text = ATLAS_HEAD.sub("", text)
    head = (f'<link rel="canonical" href="{page_origin}/codestream-atlas/">\n'
            '<script src="../imi/site-origin.js"></script>\n')
    new, n = re.subn(r"(<title>[^<]*</title>\n)", lambda m: m[1] + head, text, count=1)
    if n != 1:
        raise ValueError("Atlas page has no <title> to anchor the origin tags")
    return new


def refresh(source, output, origin="https://gigapixels.imazen.org", page_origin="https://imi.imazen.org", site_source=None):
    source, output = Path(source).resolve(), Path(output).resolve()
    root = Path(__file__).resolve().parents[2]
    if output.exists():
        raise ValueError("output must be a fresh directory")
    inputs = output.with_name(output.name + ".inputs")
    inputs.mkdir(parents=True, exist_ok=False)
    entry = (source / "gigapixel/index.html").read_text()
    old = re.search(r"bundles/([0-9a-f]{16})/gigapixel/viewer\.js", entry)[1]
    bundle = source / "bundles" / old
    for name in ("gigapixel", "src", "dist"):
        shutil.copytree(bundle / name, inputs / name)
    if (bundle / "archive").is_dir():
        shutil.copytree(bundle / "archive", inputs / "archive")
    # A retained catalog has already resolved its data URLs. Rebase these too,
    # so a refresh cannot carry a previous preview/local origin into production.
    fixtures = inputs / "gigapixel/codec-fixtures.json"
    fixtures.write_text(re.sub(r'"https?://[^/"]+/benchmarks/', '"/benchmarks/', fixtures.read_text()))
    # Keep the existing explorer template as well as its JS/WASM module graph.
    entry = entry.replace(f"../bundles/{old}/gigapixel/style.css", "./style.css")
    entry = entry.replace(f"../bundles/{old}/gigapixel/viewer.js", "./viewer.js")
    (inputs / "gigapixel/index.html").write_text(entry)
    shutil.copytree(Path(site_source) if site_source is not None else root / "tools/imij-wasm/web/site", inputs / "site")
    # Feed the publisher's origin-relative form back through the same validator.
    data = (source / "image/image.json").read_text()
    # The source stage's data origin may differ from the requested one.
    data = re.sub(r'"https?://[^/"]+/(d|p)/', r'"/\1/', data)
    manifest = inputs / "image.json"
    manifest.write_text(data)
    subprocess.run([
        "node", str(root / "tools/imij-wasm/web/scripts/package-gigapixel.mjs"),
        str(inputs), str(inputs / "dist/pkg-simd"), str(manifest), str(output), origin, page_origin,
    ], check=True)
    # Old immutable URLs remain available to already-open tabs. Never overwrite
    # bytes under a retained fingerprint, even if a new package reuses its name.
    preserve_bundles(source, output)
    shutil.copytree(source / "codestream-atlas", output / "codestream-atlas")
    atlas = output / "codestream-atlas/index.html"
    atlas.write_text(atlas_entry(atlas.read_text(), page_origin.rstrip("/")))
    shutil.copyfile(source / "_headers", output / "_headers")
    (output / "spec").mkdir()
    shutil.copyfile(output / "imi/spec.html", output / "spec/index.html")
    shutil.copyfile(output / "imi/style.css", output / "spec/style.css")
    return old


if __name__ == "__main__":
    refresh(*sys.argv[1:])
