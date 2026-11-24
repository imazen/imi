#!/usr/bin/env python3
"""Verify a deployed complete site against the local staged byte manifest."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
from pathlib import Path
import re
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen


def manifest_bytes(stage):
    stage = stage.resolve()
    rows = []
    for path in sorted(p for p in stage.rglob("*") if p.is_file() and p.name != "SHA256SUMS"):
        name = path.relative_to(stage).as_posix()
        if not path.resolve().is_relative_to(stage) or "\n" in name or "\r" in name:
            raise ValueError(f"cannot publish staged path {name!r}")
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1 << 20), b""):
                digest.update(chunk)
        rows.append(f"{digest.hexdigest()}  {name}\n")
    if not rows:
        raise ValueError(f"empty site stage: {stage}")
    return "".join(rows).encode()


def write_manifest(stage):
    manifest = manifest_bytes(stage)
    (stage / "SHA256SUMS").write_bytes(manifest)
    return manifest


def verify(stage, base):
    stage = stage.resolve()
    base = base.rstrip("/")
    manifest = (stage / "SHA256SUMS").read_bytes()
    version = hashlib.sha256(manifest).hexdigest()[:16]

    def fetch(name):
        request = Request(f"{base}/{quote(name)}?verify={version}",
                          headers={"Accept-Encoding": "identity", "Cache-Control": "no-cache",
                                   "User-Agent": "IMI-site-verifier/1.0"})
        return urlopen(request, timeout=60)

    with fetch("SHA256SUMS") as response:
        if response.read() != manifest:
            raise RuntimeError("deployed manifest differs from the staged manifest")
    entries = []
    for line in manifest.decode().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise ValueError(f"invalid manifest line: {line!r}")
        digest, name = match.groups()
        name = name.removeprefix("./")
        # Pages consumes these as configuration; they are not public resources.
        if name in ("_headers", "_redirects"):
            continue
        path = (stage / name).resolve()
        if not path.is_relative_to(stage):
            raise ValueError("manifest path escapes stage")
        entries.append((name, digest))

    hashes = dict(entries)
    if (stage / "_redirects").read_text().strip() != "/ /gigapixel/ 302":
        raise ValueError("update the verifier for the site's changed root redirect contract")
    entries.append(("", hashes["gigapixel/index.html"]))

    def verify(entry):
        name, expected = entry
        if name == "index.html":
            # Pages canonicalizes index.html to /, which redirects to the explorer.
            expected = hashes["gigapixel/index.html"]
        actual = hashlib.sha256()
        with fetch(name) as response:
            if name in ("", "index.html") and urlsplit(response.geturl()).path != "/gigapixel/":
                raise RuntimeError("root did not redirect to the explorer")
            for chunk in iter(lambda: response.read(1 << 20), b""):
                actual.update(chunk)
        if actual.hexdigest() != expected:
            raise RuntimeError(f"deployed bytes differ: {name}")

    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(verify, entries))
    print(f"Verified manifest and {len(entries)} resource/redirect responses at {base}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", type=Path, help="complete local site")
    parser.add_argument("url", nargs="?", help="deployed site origin")
    parser.add_argument("--write-manifest", action="store_true", help="inventory the local stage")
    args = parser.parse_args()
    if args.write_manifest == bool(args.url):
        parser.error("provide a deployed URL or --write-manifest")
    if args.write_manifest:
        write_manifest(args.stage)
    else:
        verify(args.stage, args.url)
