"""Run the lossless KADID JXL size collector in independent bounded groups.

Each group keeps its own resumable journal. The aggregate summary appears only
after every declared group has produced and rechecked a complete summary.
"""

import argparse
import concurrent.futures
import json
import pathlib
import subprocess

from jxl_archive_size import require, sha256, write_json


def run_group(args: argparse.Namespace, group: str) -> tuple[str, dict]:
    output = args.output / "groups" / group
    command = [
        "python3",
        str(pathlib.Path(__file__).with_name("jxl_archive_size.py")),
        str(args.encoder),
        str(args.manifest),
        str(args.source_dir),
        str(args.images),
        str(output),
        "--profile",
        args.profile,
        "--groups",
        group,
    ]
    log = args.output / f"{group}.log"
    with log.open("wb") as stream:
        result = subprocess.run(
            command, stdout=stream, stderr=subprocess.STDOUT, check=False
        )
    require(result.returncode == 0, f"{group} rc={result.returncode}: {log}")
    summary_file = output / "summary.json"
    summary = json.loads(summary_file.read_text())
    require(
        summary["profile"] == args.profile
        and summary["groups"] == 1
        and summary["images"] == 126
        and summary["encoder_sha256"] == sha256(args.encoder)
        and summary["source_manifest_sha256"] == sha256(args.manifest)
        and summary["journal_sha256"] == sha256(output / "completed.jsonl"),
        f"bad group summary: {group}",
    )
    return group, summary


def run(args: argparse.Namespace) -> None:
    require(1 <= args.workers <= 8, "workers must be 1..8")
    manifest = json.loads(args.manifest.read_text())
    require(
        manifest["references"] == 81 and manifest["distortions"] == 10125,
        "wrong KADID manifest",
    )
    all_groups = [pathlib.Path(g["reference"]).stem for g in manifest["groups"]]
    require(len(all_groups) == len(set(all_groups)) == 81, "bad group inventory")
    if args.groups:
        selected = args.groups.split(",")
        require(len(selected) == len(set(selected)), "duplicate requested group")
        require(set(selected).issubset(all_groups), "unknown requested group")
        groups = [name for name in all_groups if name in selected]
    else:
        groups = all_groups
    plan = {
        "profile": args.profile,
        "effort": 13,
        "faster_decoding": {"max": 0, "fast-decode": 4}[args.profile],
        "groups": groups,
        "workers": args.workers,
        "manifest_sha256": sha256(args.manifest),
        "encoder_sha256": sha256(args.encoder),
        "collector_sha256": sha256(
            pathlib.Path(__file__).with_name("jxl_archive_size.py")
        ),
        "batch_driver_sha256": sha256(pathlib.Path(__file__)),
    }
    args.output.mkdir(parents=True, exist_ok=True)
    plan_file = args.output / "plan.json"
    if plan_file.exists():
        require(json.loads(plan_file.read_text()) == plan, "resume plan changed")
    else:
        require(not any(args.output.iterdir()), "nonempty output without plan")
        write_json(plan_file, plan)
    (args.output / "groups").mkdir(exist_ok=True)
    completed: dict[str, dict] = {}
    failures = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        jobs = {pool.submit(run_group, args, group): group for group in groups}
        for future in concurrent.futures.as_completed(jobs):
            group = jobs[future]
            try:
                name, summary = future.result()
                completed[name] = summary
                print(f"completed {name}: {len(completed)}/{len(groups)}", flush=True)
            except Exception as error:
                failures.append(f"{group}: {error}")
    require(not failures, "; ".join(failures))
    require(set(completed) == set(groups), "incomplete group set")
    summary = {
        "profile": args.profile,
        "effort": 13,
        "faster_decoding": plan["faster_decoding"],
        "groups": len(groups),
        "images": sum(completed[name]["images"] for name in groups),
        "pixels": sum(completed[name]["pixels"] for name in groups),
        "original_png_bytes": sum(
            completed[name]["original_png_bytes"] for name in groups
        ),
        "jxl_payload_bytes": sum(
            completed[name]["jxl_payload_bytes"] for name in groups
        ),
        "encoder_sha256": plan["encoder_sha256"],
        "source_manifest_sha256": plan["manifest_sha256"],
        "group_summary_sha256": {
            name: sha256(args.output / "groups" / name / "summary.json")
            for name in groups
        },
        "scope": "individual lossless JXL payload bytes; no archive index or metadata",
    }
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary, sort_keys=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("encoder", type=pathlib.Path)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("source_dir", type=pathlib.Path)
    parser.add_argument("images", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--profile", required=True, choices=["max", "fast-decode"])
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument(
        "--groups", help="comma-separated subset for a declared qualification run"
    )
    run(parser.parse_args())
