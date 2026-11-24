#!/usr/bin/env python3
"""Measure isolated cold builds; run under run-heavy, with a fresh output path."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--encode-split", action="store_true")
    parser.add_argument("--packages", action="store_true", help="measure the pinned browser packaging pipeline")
    parser.add_argument("--packages-only", action="store_true", help="measure packaging without repeating raw builds")
    args = parser.parse_args()
    args.packages = args.packages or args.packages_only
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    revision = subprocess.check_output(
        ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], cwd=root, text=True
    ).strip()
    source_commit = subprocess.check_output(
        ["jj", "log", "-r", "heads(::@ & ~empty())", "--no-graph", "-T", "commit_id"],
        cwd=root, text=True,
    ).strip()
    assert len(source_commit) == 40, "audit requires one source commit"
    cases = [
        ("native-default", ["-p", "imij", "--lib", "--release"], {}, None),
        ("native-serial", ["-p", "imij", "--lib", "--release", "--no-default-features",
                           "--features", "std,extended"], {}, None),
        ("browser-simd", ["--manifest-path", "tools/imij-wasm/web/crate/Cargo.toml",
                          "--lib", "--target", "wasm32-unknown-unknown",
                          "--profile", "wasm-release"],
         {"RUSTFLAGS": "-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext,+mutable-globals"},
         "wasm32-unknown-unknown/wasm-release/imij_web.wasm"),
    ]
    if args.encode_split:
        for index in (2, 1):
            name, arguments, env, artifact = cases[index]
            cases.insert(index, (name + "-encode", arguments + ["--features", "imij/encode"], env, artifact))
    if args.packages:
        if args.packages_only:
            cases = []
        for suffix, features in (("encode", "imij/encode"), ("decode", "")):
            name = "package-simd-" + suffix
            cases.append((name, None, {
                "IMIJ_WASM_TARGET_DIR": str(output / name),
                "IMIJ_WASM_DIST": str(output / (name + "-dist")),
                "IMIJ_WASM_FEATURES": features,
                "IMIJ_WASM_STREAMING_EVALUATION": "0",
            }, None))
    report = {"revision": revision, "source_tree_commit": source_commit, "rustc": subprocess.check_output(
        ["rustc", "-Vv"], text=True), "cases": []}
    if args.packages:
        nightly = os.environ.get("IMIJ_WASM_NIGHTLY", "nightly-2026-09-02")
        report["packaging_rustc"] = subprocess.check_output(
            ["rustc", "+" + nightly, "-Vv"], text=True)
    for name, arguments, extra_env, artifact in cases:
        target = output / name
        command = (["cargo", "build", "--locked", "--timings", "--target-dir", str(target), *arguments]
                   if arguments is not None else ["tools/imij-wasm/web/scripts/build-wasm.sh", "simd"])
        env = os.environ.copy()
        for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR",
                    "IMIJ_WASM_FEATURES", "IMIJ_WASM_DIST", "IMIJ_WASM_TARGET_DIR",
                    "IMIJ_WASM_STREAMING_EVALUATION"):
            env.pop(key, None)
        env.update(extra_env)
        print(f"Starting {name}: {' '.join(command)}", flush=True)
        start = time.monotonic()
        with (output / f"{name}.log").open("w") as log:
            result = subprocess.run(
                ["/usr/bin/time", "-v", "-o", str(output / f"{name}.time"), *command],
                cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
            )
        timing = (output / f"{name}.time").read_text()
        row = {"name": name, "command": command, "environment": extra_env,
               "elapsed_seconds": round(time.monotonic() - start, 3),
               "max_rss_kib": int(re.search(r"Maximum resident set size \(kbytes\): (\d+)", timing)[1]),
               "exit_code": result.returncode}
        if result.returncode == 0:
            timing_html = (target / "cargo-timings" / "cargo-timing.html").read_text()
            unit_data, _ = json.JSONDecoder().raw_decode(timing_html.split("const UNIT_DATA = ", 1)[1])
            unit = next(u for u in unit_data if u["name"] == "imij")
            # Cargo's phase schema differs between the stable and pinned
            # nightly tools. Keep what that tool actually reports.
            row["imij_timing"] = {key: unit[key] for key in
                                  ("duration", "sections", "rmeta_time", "features") if key in unit}
            if args.encode_split or arguments is None:
                expected_encode = name == "native-default" or name.endswith("-encode")
                assert ("encode" in unit["features"]) == expected_encode, (name, unit["features"])
        if result.returncode == 0 and artifact:
            data = (target / artifact).read_bytes()
            row.update(artifact_bytes=len(data), artifact_sha256=hashlib.sha256(data).hexdigest())
        if result.returncode == 0 and arguments is None:
            row["packaged_artifacts"] = {}
            for suffix in ("", ".gz", ".br"):
                path = Path(extra_env["IMIJ_WASM_DIST"]) / "pkg-simd" / ("imij_bg.wasm" + suffix)
                data = path.read_bytes()
                row["packaged_artifacts"][path.name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        report["cases"].append(row)
        (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(row), flush=True)
        if result.returncode:
            raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
