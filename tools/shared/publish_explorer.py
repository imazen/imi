#!/usr/bin/env python3
"""Prepare an Explorer release, verify a Pages preview, then publish it."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
from tempfile import TemporaryDirectory
from time import time_ns
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from check_site_deployment import manifest_bytes, verify, write_manifest
from package_v4_site import package

DATA_ORIGIN = "https://gigapixels.imazen.org"
PAGE_ORIGIN = "https://imi.imazen.org"
IMMUTABLE = "public, max-age=31536000, immutable"
ROOT = Path(__file__).resolve().parents[2]


def read_json(path):
    return json.loads(path.read_text())


def captured_representation(catalog, capture):
    try:
        scene = next(item for item in catalog["datasets"] if item["id"] == capture["scene"])
        rep = scene["representations"][capture["quality"]][capture["level"]]
    except (KeyError, StopIteration) as error:
        raise ValueError("capture does not match a catalog view") from error
    if rep["kind"] != "native" or any(rep.get(key) != capture[key]
                                     for key in ("contentId", "viewId", "encodedBytes")):
        raise ValueError("capture belongs to a different native archive or view")
    return rep


def merge_ranges(catalog, capture):
    rep = captured_representation(catalog, capture)
    profile = capture["profile"]
    startup = {key: profile[key] for key in ("ranges", "viewport") if key in profile}
    if "warmup" in rep.get("startup", {}):
        startup["warmup"] = rep["startup"]["warmup"]
    rep["startup"] = startup


def merge_warmup(catalog, capture):
    rep = captured_representation(catalog, capture)
    captured_startup = capture["profile"]
    warmup = captured_startup["warmup"]
    if not re.fullmatch(r"[0-9a-f]{32}", warmup["xxh3"]):
        raise ValueError("invalid warmup byte identity")
    key = f"d/{warmup['xxh3']}/warmup.bin"
    if warmup["url"] != "/" + key:
        raise ValueError("warmup path disagrees with its byte identity")
    startup = rep.setdefault("startup", {k: v for k, v in captured_startup.items()
                                         if k in ("ranges", "viewport")})
    startup["warmup"] = warmup
    return key


def active_bundle(site):
    entry = (site / "gigapixel/index.html").read_text()
    match = re.search(r"bundles/([0-9a-f]{16})/gigapixel/viewer\.js", entry)
    if not match:
        raise ValueError("site has no fingerprinted Explorer entry")
    return site / "bundles" / match[1]


def verify_local_asset(release, key):
    asset = release / "warmup.bin"
    if not 0 < asset.stat().st_size <= 128 * 1024:
        raise ValueError("warmup must contain at most 128 KiB")
    package_path = active_bundle(release / "site") / "dist/pkg-simd"
    digest = subprocess.check_output([
        "node", "--input-type=module", "-e", """
        import {readFileSync} from 'node:fs';
        import {pathToFileURL} from 'node:url';
        const [pkg, asset] = process.argv.slice(1);
        const decoder = await import(pathToFileURL(pkg + '/imij.js'));
        decoder.initSync({module:readFileSync(pkg + '/imij_bg.wasm')});
        console.log(decoder.xxh3_128(readFileSync(asset)));
        """, str(package_path), str(asset),
    ], text=True).strip()
    if key != f"d/{digest}/warmup.bin":
        raise ValueError("local warmup bytes differ from the captured identity")
    return asset


def prepare(args):
    base, output = args.base.resolve(), args.output.resolve()
    if output.exists():
        raise ValueError(f"choose a fresh release directory: {output}")
    if bool(args.dist) != bool(args.codec_source):
        raise ValueError("provide --dist and --codec-source together")
    base_inventory = (base / "SHA256SUMS").read_bytes()
    catalog = read_json(args.catalog.resolve() if args.catalog else base / "image/image.json")
    if args.range_profile:
        merge_ranges(catalog, read_json(args.range_profile / "profile.json"))
    key = merge_warmup(catalog, read_json(args.profile / "profile.json")) if args.profile else None
    dist = args.dist.resolve() if args.dist else active_bundle(base) / "dist"
    build = base / "BUILD.json"
    revision = args.codec_source or (read_json(build).get("codec_source") if build.exists() else None)
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix=".imi-release-", dir=output.parent) as directory:
        work = Path(directory)
        candidate = work / "release"
        candidate.mkdir()
        catalog_path = work / "image.json"
        catalog_path.write_text(json.dumps(catalog))
        package(base, dist, base / "codestream-atlas", candidate / "site",
                revision or "retained", catalog_path)
        write_manifest(candidate / "site")
        (candidate / "base.SHA256SUMS").write_bytes(base_inventory)
        if key:
            (candidate / "warmup.bin").write_bytes((args.profile / key).read_bytes())
            verify_local_asset(candidate, key)
        (candidate / "publish.json").write_text(json.dumps({"warmup": key}) + "\n")
        if output.exists():
            raise ValueError(f"release appeared while packaging: {output}")
        candidate.rename(output)
    print(f"Prepared {output}\nRun just site-publish preview {shlex.quote(str(output))}")


def check_public_asset(asset, key, origin, query=""):
    request = Request(f"{DATA_ORIGIN}/{key}{query}",
                      headers={"Origin": origin, "User-Agent": "IMI-site-verifier/1.0"})
    expected = asset.read_bytes()
    with urlopen(request, timeout=20) as response:
        if response.read(len(expected) + 1) != expected:
            raise ValueError(f"public bytes differ for {key}")
        if response.headers.get_content_type() != "application/octet-stream":
            raise ValueError(f"incorrect public content type for {key}")
        if response.headers.get("Access-Control-Allow-Origin") not in ("*", origin):
            raise ValueError(f"data origin does not allow {origin}")


def publish_asset(asset, key):
    try:
        check_public_asset(asset, key, PAGE_ORIGIN, f"?publish={time_ns()}")
        return
    except HTTPError as error:
        if error.code != 404:
            raise
    endpoint = os.environ.get("IMI_R2_ENDPOINT")
    if not endpoint:
        raise ValueError("set IMI_R2_ENDPOINT and configure AWS CLI access to upload the missing warmup")
    result = subprocess.run([
        "aws", "--endpoint-url", endpoint, "s3api", "put-object", "--bucket", "gigapixels",
        "--key", key, "--body", str(asset), "--if-none-match", "*",
        "--content-type", "application/octet-stream", "--cache-control", IMMUTABLE,
    ], capture_output=True, text=True)
    if result.returncode and "PreconditionFailed" not in result.stderr:
        raise ValueError(result.stderr.strip() or "R2 upload failed")
    check_public_asset(asset, key, PAGE_ORIGIN)


def deploy(site, branch):
    result = subprocess.run(["just", "site-deploy", branch], cwd=ROOT,
                            env=dict(os.environ, IMI_SITE=str(site)), text=True,
                            stdout=subprocess.PIPE)
    print(result.stdout, end="")
    result.check_returncode()
    match = re.search(r"Deployment complete!.*?(https://[a-z0-9-]+\.imi-8zh\.pages\.dev)", result.stdout)
    if not match:
        raise ValueError("Wrangler did not return the immutable deployment URL")
    return match[1]


def publish(args):
    release = args.release.resolve()
    site = release / "site"
    key = read_json(release / "publish.json")["warmup"]
    asset = verify_local_asset(release, key) if key else None
    inventory = manifest_bytes(site)
    if inventory != (site / "SHA256SUMS").read_bytes():
        raise ValueError("staged site changed; prepare a fresh release")
    receipt = release / "preview.SHA256SUMS"
    if args.command == "preview":
        if args.branch == "production":
            raise ValueError("use the publish command for production")
        if asset:
            publish_asset(asset, key)
        url = deploy(site, args.branch)
        verify(site, url)
        if asset:
            check_public_asset(asset, key, url)
        receipt.write_bytes(inventory)
        print(f"Verified {url}\nRun just site-publish publish {shlex.quote(str(release))}")
    else:
        if not receipt.exists() or receipt.read_bytes() != inventory:
            raise ValueError("verify a preview of this release before publishing")
        request = Request(f"{PAGE_ORIGIN}/SHA256SUMS?publish={time_ns()}",
                          headers={"Cache-Control": "no-cache", "User-Agent": "IMI-site-verifier/1.0"})
        with urlopen(request, timeout=30) as response:
            if response.read() != (release / "base.SHA256SUMS").read_bytes():
                raise ValueError("production changed since prepare; use its current stage as --base")
        if asset:
            check_public_asset(asset, key, PAGE_ORIGIN)
        url = deploy(site, "production")
        verify(site, PAGE_ORIGIN)
        print(f"Live at {PAGE_ORIGIN}/gigapixel/ ({url})\nUse {site} as the next --base")


def main():
    parser = argparse.ArgumentParser(prog="just site-publish", description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    stage = commands.add_parser("prepare", help="assemble a fresh release locally")
    stage.add_argument("--profile", type=Path, help="optional directory from startup profile --warmup")
    stage.add_argument("--range-profile", type=Path, help="optional captured archive ranges to prefetch")
    stage.add_argument("--output", required=True, type=Path, help="new self-contained release directory")
    stage.add_argument("--base", type=Path, help="current production stage (defaults to IMI_SITE)",
                       default=Path(os.environ.get("IMI_SITE", Path.home() / "tmp/imi-site")))
    stage.add_argument("--dist", type=Path, help="qualified Wasm packages, if replacing the decoder")
    stage.add_argument("--codec-source", help="source revision of the replacement Wasm")
    stage.add_argument("--catalog", type=Path, help="optional replacement image.json with updated R2 archive entries")
    preview = commands.add_parser("preview", help="publish the asset and verify a Pages preview")
    preview.add_argument("release", type=Path)
    preview.add_argument("--branch", default="explorer-preview")
    commands.add_parser("publish", help="publish the verified preview to production").add_argument(
        "release", type=Path)
    args = parser.parse_args()
    try:
        (prepare if args.command == "prepare" else publish)(args)
    except (KeyError, OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"site-publish: {error}\n")


if __name__ == "__main__":
    main()
