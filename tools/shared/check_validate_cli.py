#!/usr/bin/env python3
"""Exercise actual CLI status, JSON, corruption and scratch ownership."""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

example, engine, fixture = map(lambda s: Path(s).resolve(), sys.argv[1:])
root = Path(tempfile.mkdtemp(prefix="imi-validator-cli-", dir=Path.home() / "tmp"))
source = root / "valid.imi"
subprocess.run([fixture, source, "67", "75"], check=True)
corrupt = bytearray(source.read_bytes())
psum = corrupt.index(b"pSUM")
corrupt[psum + 10] ^= 1  # first XXH3-128 digest byte in this generated fixture
bad = root / "bad.imi"
bad.write_bytes(corrupt)
for name, command in [("example", [example]), ("engine", [engine, "validate"])]:
    def run(*args):
        return subprocess.run([*command, *map(str, args)], capture_output=True, text=True)
    assert run("--help").returncode == 0
    scratch = root / (name + ".scratch")
    result = run(source, scratch, "--max-scratch-bytes", "0")
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["decoded_pixels"] == 67 * 75 and report["pixel_checksums"] == 1
    assert report["scratch_bytes"] == 0 and scratch.stat().st_size == 0
    scratch.write_bytes(b"retain existing contents")
    assert run(source, scratch).returncode == 2
    assert scratch.read_bytes() == b"retain existing contents"
    failed = run(bad, root / (name + ".bad.scratch"))
    assert failed.returncode == 2 and not failed.stdout and "checksum" in failed.stderr
    limited = run(source, root / (name + ".limit.scratch"), "--max-file-bytes", "1")
    assert limited.returncode == 2 and not limited.stdout
    refused = root / (name + ".structure.scratch")
    assert run(source, refused, "--structure-only").returncode == 2
    assert not refused.exists()
print(f"CLI validation passed; artifacts: {root}")
