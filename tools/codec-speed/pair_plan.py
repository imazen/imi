"""Generate zenbench PR33 plans over the retained, checked photo fixtures."""
import json
import pathlib
import sys

root, program, output = (pathlib.Path(p).resolve() for p in sys.argv[1:])
output.mkdir(exist_ok=False)
settings = [("imij", 0), ("imij", 50), ("imij", 80), ("zenjpeg", 50),
            ("zenjpeg", 80), ("zenpng", 1), ("zenpng", 2), ("zenpng", 7)]


def arm(name, args):
    return {"id": name, "program": str(program), "args": args,
            "timeout_seconds": 30, "revision": "see retained Cargo.lock and source.json"}


def plan(base, candidates, cases, check, warmup):
    return {"schema": 1, "baseline": base, "candidates": candidates,
            "cases": cases, "metric": {"extract": {"kind": "prefix", "prefix": "elapsed_ms="},
            "unit": "ms", "expected": 1, "divide_by": 1}, "check": check,
            "warmup": warmup, "blocks": 3, "aa": True, "deadline_seconds": 180}


for codec, setting in settings:
    cases = [{"id": image, "inputs": {"raw": str(root / f"{image}.rgb"),
             "encoded": str(root / f"{image}-{codec}-{setting}.bin")}} for image in ["1000", "1400", "1600"]]
    def dec(t):
        return arm(f"{codec}_{setting}_{t}t", ["decode", codec, str(setting), str(t), "${raw}", "${encoded}"])
    p = plan(dec(1), [dec(2 if codec == "zenpng" else 8)], cases,
             {"kind": "json", "prefix": "check=", "pointers": ["/sha256", "/bytes"]},
             "Three in-process decode warmups; one internal metric is ten allocating decodes divided by ten. Input I/O, hashing and output comparison excluded.")
    (output / f"decode-{codec}-{setting}.json").write_text(json.dumps(p, indent=2))

cases = []
for image in ["1000", "1400", "1600"]:
    inputs = {"raw": str(root / f"{image}.rgb")}
    inputs.update({f"{c}_{s}": str(root / f"{image}-{c}-{s}.bin") for c, s in settings})
    cases.append({"id": image, "inputs": inputs})
arms = [arm(f"{c}_{s}", ["encode", c, str(s), "1", "${raw}", f"${{{c}_{s}}}"]) for c, s in settings]
p = plan(arms[0], arms[1:], cases,
         {"kind": "json", "prefix": "check=", "pointers": ["/source_sha256", "/bytes", "/matches_prepared"]},
         "One full encode warmup, then one measured encode, all single-threaded. Each output must exactly equal its own retained encoded fixture outside timing. Cross-codec quality is not matched.")
(output / "encode.json").write_text(json.dumps(p, indent=2))
