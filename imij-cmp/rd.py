#!/usr/bin/env python3
"""Local artifact-backed RD cells. Heavy subprocesses share the user's lock.

Existing enc/dec commands remain available. This driver never measures native
speed. Re-running verifies completed hashes and resumes missing cells. Partial
outputs/errors remain in attempts/; unsupported cells are explicit in cases.json.

Decoded pixels are deleted once a cell's record is saved (its sha256 stays in
the record and the score file); decode the persisted encoded file to recover
them. --keep-decoded retains them.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

REPO = Path(__file__).resolve().parent.parent
SCRATCH = Path("local-scratch/zqi-expanded-rd-20261004")
LOCK = os.path.expandvars("$HOME/tmp/imij-heavy.lock")
# Shared heavy-job slots on this host (tools/heavy-slot.sh; slot 0 is LOCK).
SLOT = str(Path(__file__).resolve().parent.parent / "tools/heavy-slot.sh")
HEAVY = "run-heavy"


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fingerprint(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def save(path, value):
    path = Path(path)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, indent=2) + "\n")
    temp.replace(path)


def configs(qs, distances):
    cells = []
    for q in qs:
        for color in ("xyb", "ycocg"):
            for entropy in ("runlevel", "splithuf"):
                for stripe in (1, 8):
                    cells.append(
                        dict(
                            codec="imij",
                            quality=q,
                            color=color,
                            entropy=entropy,
                            stripe_block_rows=stripe,
                            rdoq_lambda=0,
                            effort=10,
                            threads=6,
                            experiment="tested-envelope",
                        )
                    )
            cells.append(
                dict(
                    codec="imij",
                    quality=q,
                    color=color,
                    entropy="runlevel",
                    stripe_block_rows=1,
                    rdoq_lambda=0.08,
                    effort=10,
                    threads=6,
                    experiment="rdoq-lambda-0.08",
                )
            )
        for chroma in ("444", "420"):
            cells.append(
                dict(
                    codec="jpeg",
                    quality=q,
                    chroma=chroma,
                    progressive=True,
                    auto_optimize=True,
                    aq=True,
                    quality_axis="ApproxJpegli",
                )
            )
        for chroma in ("444", "420"):
            cells.append(
                dict(
                    codec="avif",
                    quality=q,
                    chroma=chroma,
                    backend="Zenav1Svt",
                    speed=6,
                    threads=6,
                )
            )
    cells += [
        dict(codec="jxl", quality=d, quality_axis="distance", effort=7)
        for d in distances
    ]
    return cells


def load_config_grid(path):
    """Admit a frozen explicit grid; the comparator validates codec policies."""
    data = Path(path).read_bytes()
    grid = json.loads(data)
    if not isinstance(grid, list) or not grid:
        raise ValueError("config grid must be a nonempty JSON array")
    seen = []
    for config in grid:
        if not isinstance(config, dict):
            raise ValueError("every config grid entry must be an object")
        # Reject numeric aliases too: JSON 1 and 1.0 describe the same cell.
        if config in seen:
            raise ValueError("duplicate config in explicit grid")
        # Python accepts NaN/Infinity during parsing; persisted codec configs
        # must be standard JSON, which Rust's parser also requires.
        json.dumps(config, allow_nan=False)
        seen.append(config)
    return grid, hashlib.sha256(data).hexdigest()


def trellis_active(config, layouts):
    """Whether the encoder runs an AC trellis for this IMIJ cell, as imij's lossy encoder decides.

    An explicit lab `trellis` knob decides on its own ("off" never runs). Without one the
    production effort trellis runs from effort 9 (imij/src/effort.rs TRELLIS_ONE_PASS) for
    XYB with run-level AC, default 8x8 layouts and RDOQ off; imij-cmp sets no decode-speed
    tier, so run-level means the configured entropy. The driver requires `effort` and
    `entropy` on every IMIJ cell.
    """
    trellis = config.get("trellis")
    if trellis is not None:
        return trellis != "off"
    return (
        float(config["effort"]) >= 9.0
        and config["color"] == "xyb"
        and config["entropy"] == "runlevel"
        and config["rdoq_lambda"] == 0
        and all((layout["transform"], layout["sampling"]) == ("8", "full") for layout in layouts)
    )


def pixel_key(config):
    """Entropy, stripe geometry and compact syntax must preserve these pixels, unless an AC
    trellis runs: its output depends on the exact effort, stripe geometry and entropy, so those
    join the key only then."""
    layouts = config.get(
        "channel_layouts", [{"transform": "8", "sampling": "full"}] * 3
    )
    active = trellis_active(config, layouts)
    return (
        config["quality"],
        config["color"],
        config["rdoq_lambda"],
        tuple((layout["transform"], layout["sampling"]) for layout in layouts),
        # Encoder ablations change pixels; each arm is its own pixel group.
        tuple(config.get("dc_step", ())),
        config.get("dering"),
        config.get("dc_nearest"),
        # Explicit knobs as given; absent follows the production eligibility above, so an
        # inactive absent trellis groups with "off".
        config.get("trellis", "effort" if active else "off"),
        config.get("trellis_lambda1"),
        config.get("trellis_lambda2"),
        config.get("trellis_passes"),
        config.get("trellis_min_quality"),
        config.get("trellis_thorough", False),
        config.get("trellis_aq_coupling", 0),
        (float(config["effort"]), config["stripe_block_rows"], config["entropy"]) if active else None,
    )


def snapshot(binary):
    """Hash each resolved package source, including dirty local dependencies."""
    cmd = [
        "cargo",
        "metadata",
        "--offline",
        "--locked",
        "--format-version",
        "1",
        "--manifest-path",
        str(REPO / "imij-cmp/Cargo.toml"),
    ]
    metadata = json.loads(subprocess.check_output(cmd, cwd=REPO))
    packages = []
    files = {}
    for p in metadata["packages"]:
        root = Path(p["manifest_path"]).parent
        hashes = {}
        for f in sorted(root.rglob("*")):
            if not f.is_file() or any(
                s in {".git", ".jj", "target", ".claude"}
                for s in f.relative_to(root).parts
            ):
                continue
            if f.suffix not in {".rs", ".toml", ".json", ".wgsl", ".bin"}:
                continue
            hashes[str(f.relative_to(root))] = digest(f)
        packages.append(
            dict(
                name=p["name"],
                version=p["version"],
                source=p["source"],
                root=str(root),
                tree_sha256=fingerprint(hashes),
                files=hashes,
            )
        )
    for root in (REPO / "imij-cmp/src", REPO / "imij/src", REPO / "imij-gather/src"):
        for f in sorted(root.rglob("*.rs")):
            files[str(f.relative_to(REPO))] = digest(f)
    for f in (
        "imij-cmp/rd.py",
        "imij-cmp/freeze_build.py",
        "imij-cmp/Cargo.toml",
        "imij-cmp/Cargo.lock",
        "imij-bench/benches/matrix/quality.rs",
        "benchmarks/perceptual.rs",
    ):
        files[f] = digest(REPO / f)
    build_keys = {
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTUP_TOOLCHAIN",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTFLAGS",
    }
    build_keys.update(
        k
        for k in os.environ
        if k.startswith("CARGO_PROFILE_RELEASE_")
        or (
            k.startswith("CARGO_TARGET_")
            and k.endswith(("_RUSTFLAGS", "_LINKER", "_RUNNER"))
        )
    )
    return dict(
        binary_sha256=digest(binary) if binary else None,
        source_files=files,
        packages=packages,
        rustc=subprocess.check_output(["rustc", "-Vv"], cwd=REPO, text=True),
        jj_change=subprocess.check_output(
            ["jj", "log", "-r", "@", "--no-graph", "-T", "change_id"],
            cwd=REPO,
            text=True,
        ),
        commit_id=subprocess.check_output(
            ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"],
            cwd=REPO,
            text=True,
        ),
        build_command=[
            SLOT,
            HEAVY,
            "--mem",
            "12G",
            "--jobs",
            "6",
            "--",
            "cargo",
            "build",
            "--offline",
            "--release",
            "--manifest-path",
            "imij-cmp/Cargo.toml",
        ],
        build_environment={k: os.environ.get(k) for k in sorted(build_keys)},
        target_directory=metadata["target_directory"],
        resolved_features={
            node["id"]: node["features"] for node in metadata["resolve"]["nodes"]
        },
        machine=os.uname().nodename,
        date=time.strftime("%Y-%m-%d", time.gmtime()),
        lab=True,
        timing="local wall time diagnostic only",
        metric_maps="optional diagnostic subset",
    )


def verify_case(row):
    for pathkey, hashkey in [
        ("source", "source_sha256"),
        ("decoded", "decoded_sha256"),
        ("encoded", "encoded_sha256"),
    ]:
        if pathkey == "decoded" and row.get("decoded_pruned"):
            if Path(row["decoded"]).exists():
                raise ValueError(f"pruned decoded pixels reappeared: {row['decoded']}")
            continue
        if digest(Path(row[pathkey])) != row[hashkey]:
            raise ValueError(f"corrupt persisted {pathkey}: {row[pathkey]}")
    if "scores" in row:
        if digest(row["scores"]) != row["scores_sha256"]:
            raise ValueError("persisted score hash changed")
        scores = json.loads(Path(row["scores"]).read_text())
        for key in ("source_sha256", "decoded_sha256", "width", "height"):
            if scores[key] != row[key]:
                raise ValueError(f"score pair {key} differs")


class Interrupted(Exception):
    """SIGINT/SIGTERM, raised from the handler so cleanup runs outside it."""


def raise_interrupted(signum, _frame):
    raise Interrupted(signum)


class Runner:
    def __init__(self, binary, output, lock=True):
        self.binary, self.output, self.lock = str(binary), output, lock
        self.progress = output / "progress.jsonl"

    def call(self, args, label):
        (REPO / ".workongoing").write_text(
            time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            + " codecs "
            + label
            + "\n"
        )
        logs = self.output / "logs"
        logs.mkdir(exist_ok=True)
        logfile = logs / (label + "." + str(time.time_ns()) + ".log")
        command = [
            HEAVY,
            "--mem",
            "12G",
            "--jobs",
            "6",
            "--",
            self.binary,
            *map(str, args),
        ]
        if self.lock:
            command = [SLOT, *command]
        start = time.time()
        with open(logfile, "wb") as log:
            # Own session: an interrupt stops flock, run-heavy and imij-cmp.
            proc = subprocess.Popen(
                command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True
            )
            try:
                proc.wait()
            except Interrupted:
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait()
                raise
        event = dict(
            label=label,
            command=command,
            returncode=proc.returncode,
            log=str(logfile),
            log_sha256=digest(logfile),
            wall_seconds=time.time() - start,
            completed_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        )
        with open(self.progress, "a") as f:
            f.write(json.dumps(event) + "\n")
        if proc.returncode:
            raise RuntimeError(
                f"{label} failed ({proc.returncode}); exact output: {logfile}"
            )


def preserve_failed_attempts(output):
    """Explicit retry: archive failures/partial scores without altering successes."""
    attempts = output / "attempts"
    attempts.mkdir(exist_ok=True)
    archived = []
    for record in sorted((output / "records").glob("*.json")):
        row = json.loads(record.read_text())
        if row["status"] != "failed":
            continue
        target = attempts / (
            record.stem + ".failed-record." + str(time.time_ns()) + ".json"
        )
        record.rename(target)
        archived.append(
            dict(original=str(record), archived=str(target), sha256=digest(target))
        )
    archived += archive_partial_scores(output)
    save(attempts / ("retry-" + str(time.time_ns()) + ".json"), archived)


def archive_partial_scores(output):
    """Move score directories without scores.json into attempts/."""
    attempts = output / "attempts"
    attempts.mkdir(exist_ok=True)
    archived = []
    for score_dir in sorted((output / "scores").glob("*")):
        if score_dir.is_dir() and not (score_dir / "scores.json").exists():
            target = attempts / (
                score_dir.name + ".partial-score." + str(time.time_ns())
            )
            score_dir.rename(target)
            archived.append(
                dict(
                    original=str(score_dir),
                    archived=str(target),
                    files={
                        str(p.relative_to(target)): digest(p)
                        for p in target.rglob("*")
                        if p.is_file()
                    },
                )
            )
    return archived


def prune_unrecorded_decoded(output):
    """Delete decoded.rgb8 of cells whose case.json records its sha256."""
    pruned = 0
    for decoded in (output / "cells").glob("*/decoded.rgb8"):
        if (decoded.parent / "case.json").exists():
            decoded.unlink()
            pruned += 1
    return pruned


def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # One driver per output: concurrent drivers share batch.json, so a batch
    # could encode one image's cells from another image's source.
    lock = open(output / ".rd.lock", "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise RuntimeError(f"another rd.py is running on {output}") from None
    corpus = json.loads(args.corpus.read_text())
    images = corpus["images"][: args.images] if args.images else corpus["images"]
    if args.config_grid:
        grid, grid_sha256 = load_config_grid(args.config_grid)
    else:
        qs = [float(q) for q in args.qualities.split(",")]
        ds = [float(d) for d in args.distances.split(",")]
        grid = configs(qs, ds)
    if args.codecs:
        grid = [c for c in grid if c["codec"] in args.codecs.split(",")]
    if not grid:
        raise ValueError("configuration grid is empty after codec filtering")
    plan = dict(
        corpus_sha256=digest(args.corpus),
        images=images,
        grid=grid,
        maps=args.maps,
        version=1,
    )
    if args.config_grid:
        plan["config_grid"] = dict(
            path=str(args.config_grid.resolve()), sha256=grid_sha256
        )
    plan_path = output / "plan.json"
    if plan_path.exists() and json.loads(plan_path.read_text()) != plan:
        raise ValueError("resume plan differs; use another output directory")
    save(plan_path, plan)
    frozen = output / "imij-cmp"
    provenance = output / "provenance.json"
    if not provenance.exists():
        if not args.build_provenance:
            raise ValueError(
                "fresh runs require --build-provenance from freeze_build.py; never attribute an old binary to current sources"
            )
        snap = json.loads(args.build_provenance.read_text())
        if digest(args.binary) != snap["binary_sha256"]:
            raise ValueError("build provenance binary differs")
        for key in ("source_snapshot", "working_copy_patch"):
            if digest(snap[key]) != snap[key + "_sha256"]:
                raise ValueError("build provenance artifact differs: " + key)
        if (
            digest(args.build_provenance.parent / "Cargo.lock")
            != snap["source_files"]["imij-cmp/Cargo.lock"]
        ):
            raise ValueError("build provenance Cargo.lock differs")
        snap.update(
            build_provenance=str(args.build_provenance.resolve()),
            build_provenance_sha256=digest(args.build_provenance),
        )
        # Hash-checked below; a hard link avoids one binary copy per output.
        try:
            os.link(args.binary, frozen)
        except OSError:
            shutil.copy2(args.binary, frozen)
        save(provenance, snap)
        # The complete package source hash manifest accompanies Cargo.lock.
        shutil.copy2(args.build_provenance.parent / "Cargo.lock", output / "Cargo.lock")
    if digest(frozen) != json.loads(provenance.read_text())["binary_sha256"]:
        raise ValueError("frozen executable hash changed")
    runner = Runner(frozen, output, lock=not args.no_lock)
    signal.signal(signal.SIGINT, raise_interrupted)
    signal.signal(signal.SIGTERM, raise_interrupted)
    try:
        run_cells(args, output, images, grid, runner)
    except Interrupted as stop:
        # Children are stopped (Runner.call); leave no decoded pixels behind.
        # Resume re-encodes them inside their rendition batch.
        partial = archive_partial_scores(output)
        if partial:
            save(output / "attempts" / ("interrupt-" + str(time.time_ns()) + ".json"), partial)
        if not args.keep_decoded:
            pruned = prune_unrecorded_decoded(output)
            print(f"interrupted: pruned {pruned} decoded files", flush=True)
        raise SystemExit(128 + stop.args[0]) from None


def run_cells(args, output, images, grid, runner):
    if args.retry_failed:
        preserve_failed_attempts(output)
    cases = []
    failures = []
    for entry in images:
        name = entry["family"] + "-" + entry["sha256"][:12]
        source_dir = output / "sources" / name
        source_dir.parent.mkdir(exist_ok=True)
        crop_path = source_dir.parent / (name + ".json")
        save(crop_path, entry)
        if not (source_dir / "source.json").exists():
            if source_dir.exists():
                raise ValueError(
                    f"partial source output must be inspected: {source_dir}"
                )
            runner.call(
                ["source-rgb", entry["path"], crop_path, source_dir], name + "-source"
            )
        source = json.loads((source_dir / "source.json").read_text())
        if digest(source["source"]) != source["source_sha256"]:
            raise ValueError("persisted source corruption")
        w, h = source["width"], source["height"]
        # One quality group per lock lease; warm all reference metric state.
        supported = [
            c for c in grid if not (c["codec"] == "avif" and c["chroma"] == "444")
        ]
        for offset in range(0, len(supported), args.batch_size):
            jobs = []
            for config in supported[offset : offset + args.batch_size]:
                config_id = fingerprint(config)
                case_id = name + "-" + config_id[:16]
                record = output / "records" / (case_id + ".json")
                if record.exists():
                    continue
                cpath = output / "configs" / (config_id + ".json")
                cpath.parent.mkdir(exist_ok=True)
                save(cpath, config)
                cell = output / "cells" / case_id
                cell.parent.mkdir(exist_ok=True)
                # A cell without case.json is partial; one whose decoded pixels an
                # interrupt pruned is re-encoded here too (deterministic), inside this
                # batch, instead of one heavy-slot redecode call per cell.
                if cell.exists() and not (
                    (cell / "case.json").exists() and (cell / "decoded.rgb8").exists()
                ):
                    attempts = output / "attempts"
                    attempts.mkdir(exist_ok=True)
                    cell.rename(attempts / (case_id + "." + str(time.time_ns())))
                jobs.append(dict(config=str(cpath), output=str(cell)))
            if jobs:
                batch = output / "batch.json"
                save(batch, jobs)
                try:
                    runner.call(
                        ["eval-batch", source["source"], w, h, batch, output / "scores"]
                        + (["--maps"] if args.maps else []),
                        name + "-batch-" + str(offset),
                    )
                except RuntimeError as error:
                    # Individual artifacts survive; the cell loop records exact failures.
                    print(str(error), flush=True)
        pixel_groups = {}

        def check_pixels(row):
            c = row["config"]
            if c["codec"] != "imij" or row["status"] != "complete":
                return
            key = pixel_key(c)
            previous = pixel_groups.get(key)
            if previous is not None and previous != row["decoded_sha256"]:
                raise ValueError(f"equal-pixel IMIJ entropy/stripe mismatch {key}")
            pixel_groups[key] = row["decoded_sha256"]

        for config in grid:
            config_id = fingerprint(config)
            case_id = name + "-" + config_id[:16]
            record = output / "records" / (case_id + ".json")
            record.parent.mkdir(exist_ok=True)
            if record.exists():
                row = json.loads(record.read_text())
                if row["status"] == "complete":
                    if row["source_sha256"] != source["source_sha256"]:
                        raise ValueError(f"record {case_id} names another source")
                    verify_case(row)
                    check_pixels(row)
                cases.append(row)
                if row["status"] == "failed":
                    failures.append(row)
                continue
            cpath = output / "configs" / (config_id + ".json")
            cpath.parent.mkdir(exist_ok=True)
            save(cpath, config)
            base = dict(
                case_id=case_id,
                config_id=config_id,
                config=config,
                source=source["source"],
                source_sha256=source["source_sha256"],
                width=w,
                height=h,
                stride_bytes=w * 3,
                quality=config["quality"],
                image=name,
                image_class=entry["class"],
                family=entry["family"],
            )
            # SVT adapter explicitly rejects 444; record every planned cell.
            if config["codec"] == "avif" and config["chroma"] == "444":
                row = dict(
                    base,
                    status="unsupported",
                    reason="Pinned zenavif encoder_svt_rs rejects Yuv444: zenav1-svt codes Yuv420 only",
                )
                save(record, row)
                cases.append(row)
                continue
            cell = output / "cells" / case_id
            cell.parent.mkdir(exist_ok=True)
            try:
                if not (cell / "case.json").exists():
                    if cell.exists():
                        attempts = output / "attempts"
                        attempts.mkdir(exist_ok=True)
                        cell.rename(attempts / (case_id + "." + str(time.time_ns())))
                    runner.call(
                        ["encode-rgb", source["source"], w, h, cpath, cell],
                        case_id + "-encode",
                    )
                row = {**base, **json.loads((cell / "case.json").read_text())}
                if row["source_sha256"] != source["source_sha256"]:
                    raise ValueError(f"cell {case_id} was encoded from another source")
                if not Path(row["decoded"]).exists():
                    # Pruned by an interrupted run: restore from the encode.
                    runner.call(["redecode-case", cell], case_id + "-redecode")
                verify_case(row)
                row["status"] = "complete"
                check_pixels(row)
                score_key = dict(
                    source=row["source_sha256"],
                    decoded=row["decoded_sha256"],
                    width=w,
                    height=h,
                )
                if args.maps:
                    score_key["maps"] = True
                score_id = fingerprint(score_key)
                score_dir = output / "scores" / score_id
                score_dir.parent.mkdir(exist_ok=True)
                if not (score_dir / "scores.json").exists():
                    if score_dir.exists():
                        raise ValueError(
                            f"partial scoring needs inspection: {score_dir}"
                        )
                    runner.call(
                        ["score-rgb", row["source"], row["decoded"], w, h, score_dir]
                        + (["--maps"] if args.maps else []),
                        case_id + "-score",
                    )
                score = json.loads((score_dir / "scores.json").read_text())
                if (
                    score["source_sha256"] != row["source_sha256"]
                    or score["decoded_sha256"] != row["decoded_sha256"]
                ):
                    raise ValueError("score pair hashes differ")
                if args.maps:
                    for key in ("butteraugli_diffmap", "cvvdp_fhd_diffmap"):
                        if not score[key] or not Path(score[key]).is_file():
                            raise ValueError("requested diagnostic maps missing")
                row.update(
                    status="complete",
                    scores=str(score_dir / "scores.json"),
                    scores_sha256=digest(score_dir / "scores.json"),
                )
            except Exception as error:
                row = dict(base, status="failed", error=str(error))
                failures.append(row)
            if row["status"] == "complete" and not args.keep_decoded:
                row["decoded_pruned"] = True
            save(record, row)
            if row.get("decoded_pruned"):
                Path(row["decoded"]).unlink()
            cases.append(row)
            print(case_id, row["status"], flush=True)
    # Summaries once per run: rewriting them after every image re-read every
    # completed score file, quadratic in the cell count.
    save(output / "cases.json", cases)
    results = []
    for row in cases:
        if row["status"] != "complete":
            continue
        scores = json.loads(Path(row["scores"]).read_text())
        result = {
            key: row[key]
            for key in (
                "image",
                "family",
                "source_sha256",
                "width",
                "height",
                "config_id",
                "quality",
                "encoded_bytes",
                "decoded_sha256",
            )
        }
        result.update(
            content_class=row["image_class"],
            codec=row["config"]["codec"],
            config=row["config"],
        )
        result.update(
            {
                key: scores[key]
                for key in (
                    "ssimulacra2",
                    "butteraugli_max",
                    "butteraugli_p3",
                    "cvvdp_fhd_jod",
                    "vmaf_v1_fhd",
                )
            }
        )
        # Score files written before zensim joined the scorer lack the key.
        result["zensim_v0_2"] = scores.get("zensim_v0_2")
        results.append(result)
    save(output / "results.json", results)
    save(
        output / "coverage.json",
        dict(
            planned=len(images) * len(grid),
            complete=sum(c["status"] == "complete" for c in cases),
            unsupported=sum(c["status"] == "unsupported" for c in cases),
            failed=len(failures),
        ),
    )
    if failures:
        raise RuntimeError(f"{len(failures)} failed cells; see cases.json")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument(
        "--build-provenance",
        type=Path,
        help="Required for fresh runs: producer provenance from freeze_build.py",
    )
    p.add_argument("--corpus", type=Path, default=SCRATCH / "corpus.json")
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--images", type=int)
    p.add_argument(
        "--qualities",
        default=",".join(map(str, sorted(set(range(0, 101, 5)) | {1, 2, 3, 7.5}))),
    )
    p.add_argument(
        "--distances", default=",".join(map(str, [0.5, 0.75, *range(1, 26)]))
    )
    p.add_argument("--codecs")
    p.add_argument(
        "--config-grid",
        type=Path,
        help="Explicit nonempty JSON configuration array, replacing quality/distance defaults; frozen in plan.json",
    )
    p.add_argument("--maps", action="store_true")
    p.add_argument(
        "--batch-size",
        type=int,
        default=13,
        help="Cells per eval-batch process (one warm scorer per batch)",
    )
    p.add_argument(
        "--no-lock",
        action="store_true",
        help="The caller already holds a heavy-lock slot (run rd.py itself under "
        "tools/heavy-slot.sh): subprocesses use run-heavy without taking another. Only for "
        "runs that finish within minutes (e.g. one source); never hold a slot across a sweep.",
    )
    p.add_argument(
        "--keep-decoded",
        action="store_true",
        help="Retain decoded.rgb8 in each cell (default: delete after scoring)",
    )
    p.add_argument(
        "--retry-failed",
        action="store_true",
        help="Retry failed records; preserve prior records and partial output in attempts/",
    )
    run(p.parse_args())


if __name__ == "__main__":
    main()
