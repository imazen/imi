#!/usr/bin/env python3
"""The published spec contains the current wire and explicit retained grammar."""
from html.parser import HTMLParser
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = set()
        self.links = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.add(attrs["id"])
        if tag == "a" and "href" in attrs:
            self.links.append(attrs["href"])


class SpecPage(unittest.TestCase):
    def test_current_and_retained_specifications_have_distinct_anchors(self):
        with tempfile.TemporaryDirectory(dir=Path.home() / "tmp", prefix="imi-v4-spec-") as work:
            output = Path(work) / "spec.html"
            subprocess.run(["python3", str(ROOT / "tools/imij-wasm/web/site/build-spec-page.py"),
                            str(output)], check=True, capture_output=True)
            text = output.read_text()
            page = Links()
            page.feed(text)
            self.assertIn("v4-overview", page.ids)
            self.assertIn("legacy-spec", page.ids)
            self.assertTrue(any(value.startswith("v4-6") for value in page.ids))
            self.assertIn("XXH3-128", text)
            self.assertIn("Typed model definitions", text)
            self.assertIn("independent <code>imij-oracle</code> decoder covers V2/V3", text)
            self.assertTrue(any(link.startswith("https://github.com/imazen/zqi/blob/main/")
                                for link in page.links))
            for link in page.links:
                if link.startswith("#v4-"):
                    self.assertIn(link[1:], page.ids, link)
            self.assertLess(text.index('id="v4-overview"'), text.index('id="legacy-spec"'))


if __name__ == "__main__":
    unittest.main()
