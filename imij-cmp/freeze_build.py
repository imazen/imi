#!/usr/bin/env python3
"""Build once and preserve producer identity before any evaluation run."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import rd


def build_executable(log, target_directory):
    """Use the build's artifact record; never adopt a stale native executable."""
    executables = []
    for line in log.read_text().splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("target", {}).get("name") == "imij-cmp"
            and message.get("executable")
        ):
            executables.append(Path(message["executable"]).resolve())
    expected = (Path(target_directory) / "release/imij-cmp").resolve()
    if len(executables) != 1 or executables[0] != expected:
        raise ValueError(
            "build did not emit the expected native imij-cmp executable; cross-target configuration is unsupported"
        )
    return executables[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    if os.environ.get("CARGO_BUILD_TARGET"):
        raise ValueError(
            "CARGO_BUILD_TARGET is unsupported by this native comparator freezer"
        )
    snap = rd.snapshot(None)
    archive_path = output / "local-source-snapshot.tar.gz"
    with tarfile.open(archive_path, "w:gz") as archive:
        for package in snap["packages"]:
            if package["source"] is not None:
                continue
            for name, expected in package["files"].items():
                path = Path(package["root"]) / name
                if rd.digest(path) != expected:
                    raise ValueError("source changed while snapshotting")
                archive.add(path, arcname=package["name"] + "/" + name, recursive=False)
        for name, expected in snap["source_files"].items():
            path = rd.REPO / name
            if rd.digest(path) != expected:
                raise ValueError("harness changed while snapshotting")
            archive.add(path, arcname="repo/" + name, recursive=False)
    patch = output / "working-copy.patch"
    patch.write_bytes(
        subprocess.check_output(
            ["jj", "diff", "-r", snap["commit_id"], "--git"], cwd=rd.REPO
        )
    )
    command = snap["build_command"]
    command.insert(command.index("--release"), "--locked")
    command.insert(
        command.index("--release"), "--message-format=json-render-diagnostics"
    )
    log = output / "build.log"
    with log.open("wb") as stream:
        subprocess.run(
            command, cwd=rd.REPO, stdout=stream, stderr=subprocess.STDOUT, check=True
        )
    after = rd.snapshot(None)
    for key in (
        "source_files",
        "packages",
        "resolved_features",
        "build_environment",
        "target_directory",
    ):
        if after[key] != snap[key]:
            raise ValueError("source/build environment changed during compile: " + key)
    binary = output / "imij-cmp"
    shutil.copy2(build_executable(log, snap["target_directory"]), binary)
    shutil.copy2(rd.REPO / "imij-cmp/Cargo.lock", output / "Cargo.lock")
    snap.update(
        binary_sha256=rd.digest(binary),
        source_snapshot=str(archive_path),
        source_snapshot_sha256=rd.digest(archive_path),
        working_copy_patch=str(patch),
        working_copy_patch_sha256=rd.digest(patch),
        build_log=str(log),
        build_log_sha256=rd.digest(log),
    )
    rd.save(output / "provenance.json", snap)
    print(
        json.dumps(
            dict(
                binary=str(binary),
                binary_sha256=snap["binary_sha256"],
                commit_id=snap["commit_id"],
            )
        )
    )


if __name__ == "__main__":
    main()
