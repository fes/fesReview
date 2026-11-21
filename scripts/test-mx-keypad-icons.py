#!/usr/bin/env python3
"""Validate the standalone MX Creative review icon assets."""

from pathlib import Path
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
ICON_DIR = ROOT / "assets" / "mx-keypad-icons"
EXPECTED = {
    "back-1s.svg": "BACK 1 SEC",
    "dictate.svg": "DICTATE",
    "first-frame.svg": "FIRST FRAME",
    "forward-1s.svg": "FORWARD 1 SEC",
    "last-frame.svg": "LAST FRAME",
    "play-pause.svg": "PLAY / PAUSE",
    "screenshot.svg": "SCREENSHOT",
}
SVG_NS = "http://www.w3.org/2000/svg"


class MXKeypadIconTests(unittest.TestCase):
    def test_exact_asset_set(self):
        self.assertEqual(set(path.name for path in ICON_DIR.glob("*.svg")), set(EXPECTED))

    def test_icons_are_small_app_independent_svg(self):
        blocked_tags = {"script", "image", "foreignObject", "use", "style"}
        for name, label in EXPECTED.items():
            path = ICON_DIR / name
            with self.subTest(asset=name):
                payload = path.read_text(encoding="utf-8")
                root = ET.fromstring(payload)
                self.assertEqual(root.tag, f"{{{SVG_NS}}}svg")
                self.assertEqual(root.attrib["width"], "480")
                self.assertEqual(root.attrib["height"], "480")
                self.assertEqual(root.attrib["viewBox"], "0 0 480 480")
                self.assertTrue(256 <= path.stat().st_size <= 8192)
                self.assertIn("#07151b", payload.lower())
                self.assertIn("#36d8f5", payload.lower())
                self.assertIn(label, payload)
                self.assertIsNotNone(root.find(f"{{{SVG_NS}}}title"))
                self.assertIsNotNone(root.find(f"{{{SVG_NS}}}desc"))
                tags = {element.tag.rsplit("}", 1)[-1] for element in root.iter()}
                self.assertTrue(tags.isdisjoint(blocked_tags))
                self.assertNotIn("url(", payload.lower())
                self.assertNotIn("href=", payload.lower())


if __name__ == "__main__":
    unittest.main()
