#!/usr/bin/env python3
"""Compare identical visible windows, native RGB group hashes and final canvases."""
import json
from pathlib import Path
import subprocess
import sys

web, baseline, candidate, destination, engines = sys.argv[1:]
out = Path(destination)
out.mkdir()
results = []
for engine in engines.split(","):
    for level, zoom, position in [("full", "100", "center"), ("full", "50", "center"), ("half", "100", "center"), ("full", "100", "end")]:
        arms = []
        for name, url in [("before", baseline), ("after", candidate)]:
            folder = out / f"{engine}-{level}-{zoom}-{position}-{name}"
            command = ["node", "tools/imij-wasm/web/scripts/check-viewer-scaling.mjs",
                       web, url, str(folder), engine, "milkyway", level,
                       "lossless", zoom, "900x800", position]
            with (out / "commands.jsonl").open("a") as log:
                log.write(json.dumps(command) + "\n")
            subprocess.run(command, check=True)
            rows = json.loads((folder / "results.json").read_text())["rows"]
            keyed = {(r["workers"], r["trial"]): r for r in rows}
            expected = {(w, t) for w in ["auto", "1", "4", "8"] for t in range(3)}
            if len(rows) != 12 or set(keyed) != expected:
                raise RuntimeError(f"incomplete trial product: {folder}")
            arms.append(keyed)
        for key, a in arms[0].items():
            b = arms[1][key]
            for label, old, new in [
                ("view", a["state"]["view"], b["state"]["view"]),
                ("representation", a["state"]["representation"], b["state"]["representation"]),
                ("pixels", a["last"]["pixels"], b["last"]["pixels"]),
                ("native groups", a["tileHashes"], b["tileHashes"]),
                ("canvas", a["renderSha256"], b["renderSha256"]),
            ]:
                if old != new or old in [None, "", {}]:
                    raise RuntimeError(f"{engine}/{level}/{zoom}/{key}: {label} differs")
        result = {"engine": engine, "level": level, "zoom": zoom, "position": position, "exact_pairs": 12}
        results.append(result)
        (out / "results.json").write_text(json.dumps(results, indent=2))
        print(result, flush=True)
