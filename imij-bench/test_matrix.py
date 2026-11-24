"""Analytic checks of rate–distortion integration, without codec fixtures."""
import math
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest

from matrix import bd_rate, compare_rd, rd_frontier, timing_status


class TimingValidationTests(unittest.TestCase):
    def test_wall_limit_can_stop_rounds_without_unreliable_flag(self):
        report = {"unreliable": False, "comparisons": [
            {"group_name": "early stop", "completed_rounds": 4,
             "analyses": [["before", "after", {"n_samples": 4}]]}]}
        self.assertFalse(timing_status(report, 24)["passed"])

    def test_complete_rounds_still_require_paired_samples(self):
        comparison = {"group_name": "filtered", "completed_rounds": 24,
                      "analyses": [["before", "after", {"n_samples": 20}]]}
        report = {"unreliable": False, "comparisons": [comparison]}
        self.assertFalse(timing_status(report, 24)["passed"])
        comparison["analyses"][0][2]["n_samples"] = 24
        self.assertTrue(timing_status(report, 24)["passed"])
        report["unreliable"] = True
        self.assertFalse(timing_status(report, 24)["passed"])

    def test_missing_measurements_are_not_success(self):
        self.assertFalse(timing_status({}, 24)["passed"])


class RateDistortionTests(unittest.TestCase):
    def test_identical_curve(self):
        curve = [(10, 80), (20, 120), (40, 400)]
        self.assertAlmostEqual(bd_rate(curve, curve)["percent"], 0)

    def test_constant_rate_ratio_on_partial_overlap(self):
        # log(rate) = quality / 10. Integrating either line over [10, 20]
        # must give the known 25% rate penalty, without extrapolation.
        base = [(q, math.exp(q / 10)) for q in (0, 10, 20)]
        candidate = [(q, 1.25 * math.exp(q / 10)) for q in (10, 20, 30)]
        result = bd_rate(base, candidate)
        self.assertAlmostEqual(result["percent"], 25)
        self.assertEqual(result["quality_range"], [10, 20])
        self.assertEqual(result["sampled_quality_ceilings"], [20, 30])

    def test_dominated_points_and_duplicates(self):
        points = [(10, 100), (10, 100), (9, 100), (10, 110), (20, 200)]
        self.assertEqual(rd_frontier(points), [(10, math.log(100)), (20, math.log(200))])

    def test_no_overlap_or_quality_range(self):
        for candidate in ([(20, 100), (30, 200)], [(5, 100), (5, 200)]):
            with self.assertRaises(ValueError):
                bd_rate([(0, 100), (10, 200)], candidate)

    def test_invalid_metric_or_rate(self):
        for point in ((math.nan, 100), (math.inf, 100), (10, 0), (10, -1), (10, math.inf)):
            with self.assertRaises(ValueError):
                rd_frontier([point])

    def scored_cases(self, directory, settings):
        records = []
        for variant in (0, 1):
            for quality in (10, 20):
                stats = directory / f"{variant}-{quality}.json"
                stats.write_text(json.dumps({
                    "ssimulacra2": quality,
                    "cvvdp_fhd_jod": quality / 3,
                    "vmaf_v1_fhd": None,
                    "metric_settings": settings(variant),
                }))
                records.append({"stats": str(stats), "case": {
                    "pixels": "same-reference.rgb", "comparison": "one-image",
                    "encoded_bytes": (100 + quality) * (1 + variant * 0.25),
                    "profile": {"fdct": variant, "quality": quality, "name": "test"},
                }})
        (directory / "index.json").write_text(json.dumps(records))
        return SimpleNamespace(scores=directory, output=directory / "bd.json",
                               axis="fdct", baseline="0")

    def test_missing_metric_does_not_invent_a_score(self):
        with TemporaryDirectory() as tmp:
            args = self.scored_cases(Path(tmp), lambda _: {"display": "FHD"})
            compare_rd(args)
            result = json.loads(args.output.read_text())["comparisons"][0]
            self.assertEqual(result["metric_settings"], {"display": "FHD"})
            self.assertAlmostEqual(result["metrics"]["cvvdp_fhd_jod"]["percent"], 25)
            self.assertIn("unavailable", result["metrics"]["vmaf_v1_fhd"])

    def test_incompatible_viewing_conditions_are_rejected(self):
        with TemporaryDirectory() as tmp:
            args = self.scored_cases(Path(tmp), lambda v: {"display": ["FHD", "4K"][v]})
            with self.assertRaisesRegex(ValueError, "metric settings differ"):
                compare_rd(args)
            self.assertFalse(args.output.exists())


if __name__ == "__main__":
    unittest.main()
