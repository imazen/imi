#!/usr/bin/env python3
"""Atlas bundles retain both wire specifications and their source links."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
BUNDLE = Path(__file__).with_name("bundle.py")


class BundleTests(unittest.TestCase):
    def setUp(self):
        scratch = Path.home() / "tmp"
        scratch.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="imi-v4-atlas-", dir=scratch)
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)
        self.traces = self.base / "traces"
        self.traces.mkdir()
        (self.traces / "fixture-v4.json").write_text(json.dumps({
            "schema": "zen-codestream-trace/0", "codec": "imi",
            "nodes": [{"spec": "V4:6.1", "label": "XXH3-128 object digest"}],
        }))

    def run_bundle(self, spec, output):
        return subprocess.run([sys.executable, str(BUNDLE), str(self.traces),
                               str(spec), str(output)], text=True, capture_output=True)

    def test_both_specs_and_links_survive_bundling(self):
        output = self.base / "data.js"
        result = self.run_bundle(ROOT / "SPEC.md", output)
        self.assertEqual(result.returncode, 0, result.stderr)
        text = output.read_text()
        self.assertTrue(text.startswith("window.ATLAS="))
        atlas = json.loads(text[len("window.ATLAS="):].strip().removesuffix(";"))
        for section in ["1.0", "V4:3", "V4:4", "V4:5.1", "V4:6.1"]:
            self.assertTrue(atlas["spec"][section]["text"], section)
        self.assertIn("[rank/select implementation](imij/src/v4/layout.rs)",
                      atlas["spec"]["V4:5.1"]["text"])
        self.assertEqual(atlas["files"][0]["nodes"][0]["spec"], "V4:6.1")

    def test_missing_v4_spec_fails_before_output(self):
        spec = self.base / "SPEC.md"
        shutil.copyfile(ROOT / "SPEC.md", spec)
        output = self.base / "missing.js"
        result = self.run_bundle(spec, output)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SPEC_V4.md", result.stderr)
        self.assertFalse(output.exists())

    def test_viewer_scripts_parse(self):
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required for the Atlas source check")
        html = BUNDLE.with_name("index.html").read_text()
        scripts = re.findall(r"<script(?:\s[^>]*)?>([\s\S]*?)</script>", html)
        inline = [script for script in scripts if script.strip()]
        self.assertTrue(inline, "viewer has no inline program")
        for source in ["v4-grid.js", "wasi-worker.js"]:
            self.assertIn(f'src="{source}"' if source == "v4-grid.js" else source, html)
            result = subprocess.run([node, "--check", str(BUNDLE.with_name(source))],
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        for i, script in enumerate(inline):
            path = self.base / f"viewer-{i}.mjs"
            path.write_text(script)
            result = subprocess.run([node, "--check", str(path)], text=True,
                                    capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
