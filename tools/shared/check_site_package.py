#!/usr/bin/env python3
"""Check immutable identity and preservation using an explicit complete stage."""
from datetime import datetime
import json
from pathlib import Path
import re
import shutil
import sys
from refresh_site_docs import atlas_entry, preserve_bundles, refresh

source, work = map(lambda p: Path(p).resolve(), sys.argv[1:])
work.mkdir(parents=True, exist_ok=False)
# The check exercises a documentation update. Make that input change explicit
# even when the supplied stage already contains the current documentation.
# Content-addressed identical inputs correctly reuse an existing bundle ID.
site_source = work / "changed-docs"
shutil.copytree(Path(__file__).resolve().parents[1] / "imij-wasm/web/site", site_source)
with (site_source / "index.html").open("a") as page:
    page.write("\n<!-- package preservation document-change fixture -->\n")
versions = []
for name, origin in [("production", "https://gigapixels.imazen.org"),
                     ("alternate", "https://tiles.example.invalid")]:
    output = work / name
    old = refresh(source, output, origin, site_source=site_source)
    new = set(p.name for p in (output / "bundles").iterdir()) - set(
        p.name for p in (source / "bundles").iterdir())
    assert len(new) == 1
    version = new.pop()
    versions.append(version)
    bundle = output / "bundles" / version
    assert (output / "image/image.json").read_bytes() == (bundle / "image/image.json").read_bytes()
    catalog = (output / "image/image.json").read_text()
    assert '"/d/' not in catalog and f'"{origin}/d/' in catalog
    json.loads(catalog)
    # Every data path (tiles, previews, benchmark files) resolves against the data origin.
    gallery = (output / "image/gallery.json").read_text()
    assert '"/p/' not in catalog and '"/p/' not in gallery and f'"{origin}/p/' in gallery
    fixtures = (bundle / "gigapixel/codec-fixtures.json").read_text()
    assert '"/benchmarks/' not in fixtures and f'"{origin}/benchmarks/' in fixtures
    for page in ("imi/benchmarks.html", "imi/encode.html", "imi/site-origin.js", "404.html"):
        text = (output / page).read_text()
        assert "__PAGE_ORIGIN__" not in text and "__DATA_ORIGIN__" not in text, page
    assert f"'{origin}'" in (output / "imi/site-origin.js").read_text()
    assert '<link rel="canonical" href="https://imi.imazen.org/imi/benchmarks">' in (output / "imi/benchmarks.html").read_text()
    build_times = []
    for entry in ("gigapixel/index.html", "index.html"):
        text = (output / entry).read_text()
        assert '<link rel="canonical" href="https://imi.imazen.org/gigapixel/">' in text and "imi/site-origin.js" in text, entry
        stamp = re.search(r'<summary>Technical details<small id="viewer-build"> · Viewer built '
                          r'<time datetime="([^"]+)">([^<]+)</time></small></summary>', text)
        if "<summary>Technical details" in text:
            assert stamp, entry
            value = datetime.strptime(stamp[1], "%Y-%m-%dT%H:%M:%S.%fZ")
            assert stamp[2] == value.strftime("%Y-%m-%d %H:%M UTC"), entry
            build_times.append(value)
    assert not build_times or len(build_times) == 2 and build_times[0] == build_times[1]
    assert (output / "imi/validation.html").read_bytes() == (bundle / "imi/validation.html").read_bytes()
    assert f"bundles/{version}/gigapixel/viewer.js" in (output / "gigapixel/index.html").read_text()
    for path in (source / "bundles").rglob("*"):
        if path.is_file():
            assert path.read_bytes() == (output / path.relative_to(source)).read_bytes(), path
    for path in (source / "codestream-atlas").rglob("*"):
        if path.is_file() and path.name != "index.html":
            assert path.read_bytes() == (output / path.relative_to(source)).read_bytes(), path
    # The retained Atlas page only gains the canonical link and origin script.
    atlas = (source / "codestream-atlas/index.html").read_text()
    assert (output / "codestream-atlas/index.html").read_text() == atlas_entry(atlas, "https://imi.imazen.org")
    for path in (source / "bundles" / old / "dist").rglob("*.wasm"):
        assert path.read_bytes() == (bundle / path.relative_to(source / "bundles" / old)).read_bytes()
assert versions[0] != versions[1], "changing deployment URL bytes must change the immutable identity"
collision = work / "collision"
path = next(p for p in (source / "bundles").rglob("*") if p.is_file())
target = collision / path.relative_to(source)
target.parent.mkdir(parents=True)
target.write_bytes(b"different bytes under an existing immutable name")
try:
    preserve_bundles(source, collision)
except ValueError as error:
    assert "immutable bundle collision" in str(error)
else:
    raise AssertionError("an existing immutable name accepted different bytes")
print(f"Package identity, page inclusion and retained binary checks passed: {versions}")
