#!/usr/bin/env python3
"""Retain and compare hosted review state without changing GitHub or local refs.

Snapshots belong outside git. Head/base commit IDs may advance normally; branch
names, repositories, review state and existing discussion records must survive.
Raw mergeability is reported separately: GitHub's null means not yet calculated.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys

import xxhash


def api(repository, path, pages=False):
    command = ["gh", "api", f"repos/{repository}/{path}"]
    if pages:
        command += ["--paginate", "--slurp"]
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    value = json.loads(result.stdout)
    return [item for page in value for item in page] if pages else value


def digest(value):
    return xxhash.xxh3_128(value.encode()).hexdigest()


def threads(repository, number):
    owner, name = repository.split("/", 1)
    query = """query($owner:String!,$name:String!,$number:Int!,$cursor:String) {
      repository(owner:$owner,name:$name) { pullRequest(number:$number) {
        reviewThreads(first:100,after:$cursor) {
          nodes { id isResolved resolvedBy { login } }
          pageInfo { hasNextPage endCursor }
        }
      } }
    }"""
    cursor = None
    result = {}
    while True:
        command = ["gh", "api", "graphql", "-f", "query=" + query,
                   "-f", "owner=" + owner, "-f", "name=" + name,
                   "-F", f"number={number}"]
        if cursor is not None:
            command += ["-f", "cursor=" + cursor]
        response = json.loads(subprocess.run(command, capture_output=True,
                                             text=True, check=True).stdout)
        if response.get("errors"):
            raise RuntimeError("review thread query failed")
        page = response["data"]["repository"]["pullRequest"]["reviewThreads"]
        result.update((node["id"], node) for node in page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return result
        cursor = page["pageInfo"]["endCursor"]


def record(item, kind):
    fields = ["id", "node_id", "created_at", "submitted_at", "state", "path",
              "line", "original_line", "side", "start_line", "start_side",
              "original_commit_id", "in_reply_to_id"]
    result = {key: item[key] for key in fields if key in item}
    result["author"] = item.get("user", {}).get("login")
    result["body_xxh3_128"] = digest(item.get("body") or "")
    result["kind"] = kind
    # Current line/commit positions can move when a head advances; original
    # positions and the authored review text remain the continuity contract.
    for key in ("line", "side", "start_line", "start_side"):
        result.pop(key, None)
    return result


def inspect(repository, item):
    number = item["number"]
    item = api(repository, f"pulls/{number}")
    result = {key: item[key] for key in
              ("number", "node_id", "title", "state", "draft", "created_at",
               "closed_at", "merged_at")}
    result["author"] = item["user"]["login"]
    result["body_xxh3_128"] = digest(item.get("body") or "")
    for side in ("base", "head"):
        ref = item[side]
        result[side] = {"ref": ref["ref"], "sha": ref["sha"],
                        "repository": ref["repo"]["full_name"] if ref["repo"] else None,
                        "repo_id": ref["repo"]["id"] if ref["repo"] else None}
    result["assignees"] = sorted(user["login"] for user in item["assignees"])
    result["requested_reviewers"] = sorted(user["login"] for user in item["requested_reviewers"])
    result["requested_teams"] = sorted(team["id"] for team in item["requested_teams"])
    result["mergeable"] = item["mergeable"]
    result["mergeable_state"] = item["mergeable_state"]
    head = result["head"]
    if head["repository"] is None:
        raise RuntimeError(f"PR {number}: head repository is missing")
    live_head = api(head["repository"], "git/ref/heads/" + head["ref"])
    if live_head["object"]["sha"] != head["sha"]:
        raise RuntimeError(f"PR {number}: live branch and PR head differ; retry a fresh snapshot")
    result["records"] = {}
    paths = {"conversation": f"issues/{number}/comments?per_page=100",
             "inline": f"pulls/{number}/comments?per_page=100",
             "review": f"pulls/{number}/reviews?per_page=100"}
    for kind, path in paths.items():
        for entry in api(repository, path, pages=True):
            result["records"][f"{kind}:{entry['id']}"] = record(entry, kind)
    result["threads"] = threads(repository, number)
    return str(number), result


def collect(repository):
    items = api(repository, "pulls?state=open&per_page=100", pages=True)
    with ThreadPoolExecutor(max_workers=4) as pool:
        prs = dict(pool.map(lambda item: inspect(repository, item), items))
    return {"schema": 2, "repository": repository,
            "checked_at": datetime.now(timezone.utc).isoformat(), "prs": prs}


def compare(before, after, commit_map=None):
    failures = []
    if before["repository"] != after["repository"]:
        return ["snapshot schema or repository differs"]
    if before["schema"] != after["schema"]:
        # No digest is converted or relabelled. A legacy snapshot with no
        # discussion has only structural state to compare across this change.
        if (before["schema"], after["schema"]) != (1, 2):
            return ["snapshot schema or repository differs"]
        if any(pr["records"] for pr in before["prs"].values()):
            return ["legacy baseline has discussion digests; capture an XXH3 baseline before mutation"]
    exact = ("number", "node_id", "title", "state", "draft", "created_at",
             "closed_at", "merged_at", "author", "assignees",
             "requested_reviewers", "requested_teams")
    for number, old in before["prs"].items():
        new = after["prs"].get(number)
        if new is None:
            failures.append(f"PR {number}: no longer open")
            continue
        for key in exact:
            if old[key] != new[key]:
                failures.append(f"PR {number}: {key} changed")
        for side in ("base", "head"):
            for key in ("ref", "repo_id"):
                if old[side][key] != new[side][key]:
                    failures.append(f"PR {number}: {side}.{key} changed")
        old_head = (commit_map or {}).get(old["head"]["sha"], old["head"]["sha"])
        if old_head != new["head"]["sha"]:
            comparison = api(new["head"]["repository"],
                             f"compare/{old_head}...{new['head']['sha']}")
            if comparison["status"] not in ("ahead", "identical"):
                failures.append(f"PR {number}: head update is not a fast-forward")
        for key, value in old["records"].items():
            if new["records"].get(key) != value:
                failures.append(f"PR {number}: existing {key} changed or missing")
        for key, value in old["threads"].items():
            if new["threads"].get(key) != value:
                failures.append(f"PR {number}: existing review thread changed or missing")
        if old["mergeable"] is True and new["mergeable"] is False:
            failures.append(f"PR {number}: previously mergeable, now conflicting")
        if old["mergeable"] is True and new["mergeable"] is None:
            failures.append(f"PR {number}: mergeability pending; retry a fresh snapshot")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository")
    parser.add_argument("output", type=Path)
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--commit-map", type=Path,
                        help="explicit old-to-new commit map for an authorized history rewrite")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; retain earlier snapshots")
    baseline = json.loads(args.compare.read_text()) if args.compare else None
    snapshot = collect(args.repository)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation also protects against a concurrent collector.
    with args.output.open("x") as stream:
        json.dump(snapshot, stream, indent=2, sort_keys=True)
        stream.write("\n")
    commit_map = json.loads(args.commit_map.read_text()) if args.commit_map else None
    failures = compare(baseline, snapshot, commit_map) if baseline else []
    print(json.dumps({"open_prs": len(snapshot["prs"]),
                      "discussion_records": sum(len(pr["records"]) for pr in snapshot["prs"].values()),
                      "mergeability": {n: pr["mergeable"] for n, pr in snapshot["prs"].items()},
                      "failures": failures}))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
