"""Preserve a fresh source-owned reader comparison build before measurement."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile


def require(value, message):
    if not value:
        raise RuntimeError(message)


def sha(path):
    return hashlib.file_digest(path.open("rb"), "sha256").hexdigest()


def retain(root, revision):
    source = root / "zqi"
    require(len(revision) == 40 and all(c in "0123456789abcdef" for c in revision), "revision")
    require((root / "build.rc").read_text().strip() == "0", "build did not pass")
    files = {}
    with tarfile.open(root / "source.tar") as archive:
        for entry in archive:
            if not entry.isfile():
                continue
            path = Path(entry.name)
            require(not path.is_absolute() and ".." not in path.parts, "archive path")
            original = archive.extractfile(entry)
            require(original is not None, "missing archive member")
            digest = hashlib.file_digest(original, "sha256").hexdigest()
            require(sha(source / path) == digest, f"source changed: {path}")
            files[str(path)] = digest
    records = [json.loads(line) for line in (root / "build.jsonl").read_text().splitlines()]
    require(records[-1] == {"reason": "build-finished", "success": True}, "incomplete Cargo stream")
    artifacts = []
    for name, kind, manifest, features in [
        ("imij", "lib", "imij/Cargo.toml", ["avx512", "default", "extended", "rayon", "std"]),
        ("pair_probe", "bin", "tools/codec-speed/Cargo.toml", []),
    ]:
        found = [r for r in records if r.get("reason") == "compiler-artifact"
                 and r["target"]["name"] == name and r["target"]["kind"] == [kind]]
        require(len(found) == 1, f"expected one {name}")
        record = found[0]
        require(record["fresh"] is False, f"reused {name}")
        require(Path(record["manifest_path"]) == source / manifest, "foreign source")
        require(Path(record["target"]["src_path"]).is_relative_to(source), "foreign crate root")
        require(sorted(record["features"]) == features, "features changed")
        require(record["profile"]["opt_level"] == "3" and not record["profile"]["test"], "profile")
        for path in record["filenames"]:
            require(Path(path).is_relative_to(root / "target-tool"), "shared target")
        artifacts.append(record)
    destination = root / "binaries"
    destination.mkdir()
    copies = {}
    for record in artifacts:
        for filename in record["filenames"]:
            path = Path(filename)
            target = destination / path.name
            require(not target.exists(), "duplicate artifact")
            shutil.copy2(path, target)
            require(sha(path) == sha(target), "copy mismatch")
            copies[str(target.relative_to(root))] = sha(target)
    fingerprints = root / "target-tool/release/.fingerprint"
    for folder in fingerprints.iterdir():
        if folder.name.startswith(("imij-", "imij-codec-speed-")):
            for path in folder.iterdir():
                if path.is_file():
                    target = destination / "fingerprints" / folder.name / path.name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
                    require(sha(path) == sha(target), "fingerprint copy")
                    copies[str(target.relative_to(root))] = sha(target)
    proof = {"source": revision, "source_archive": sha(root / "source.tar"),
             "files": files, "artifacts": artifacts, "copies": copies,
             "build_log": sha(root / "build.log"), "build_jsonl": sha(root / "build.jsonl")}
    with (root / "build-proof.json").open("x") as out:
        json.dump(proof, out, indent=2)
    print(json.dumps({"source_files": len(files), "copies": len(copies),
                      "proof_sha256": sha(root / "build-proof.json")}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=lambda p: Path(p).resolve())
    parser.add_argument("revision")
    args = parser.parse_args()
    retain(args.root, args.revision)
