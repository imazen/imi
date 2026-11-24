import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
import rd
import freeze_build


class Artifacts(unittest.TestCase):
    def test_resume_rejects_modified_pixels_and_scores(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            row = dict(width=8, height=8)
            for key in ("source", "decoded", "encoded"):
                p = root / key
                p.write_bytes(b"pixels-" + key.encode())
                row[key] = str(p)
                row[key + "_sha256"] = rd.digest(p)
            score = root / "scores.json"
            rd.save(
                score,
                {
                    key: row[key]
                    for key in ("width", "height", "source_sha256", "decoded_sha256")
                },
            )
            row.update(scores=str(score), scores_sha256=rd.digest(score))
            rd.verify_case(row)
            original = score.read_bytes()
            score.write_bytes(original + b" ")
            with self.assertRaisesRegex(ValueError, "score hash"):
                rd.verify_case(row)
            score.write_bytes(original)
            changed = json.loads(original)
            changed["width"] = 9
            rd.save(score, changed)
            row["scores_sha256"] = rd.digest(score)
            with self.assertRaisesRegex(ValueError, "width differs"):
                rd.verify_case(row)
            Path(row["decoded"]).write_bytes(b"bad")
            with self.assertRaisesRegex(ValueError, "decoded"):
                rd.verify_case(row)

    def test_pruned_decoded_pixels_verify_by_hash_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            row = dict(width=8, height=8, decoded_pruned=True)
            for key in ("source", "decoded", "encoded"):
                p = root / key
                p.write_bytes(b"pixels-" + key.encode())
                row[key] = str(p)
                row[key + "_sha256"] = rd.digest(p)
            Path(row["decoded"]).unlink()
            rd.verify_case(row)
            Path(row["decoded"]).write_bytes(b"pixels-decoded")
            with self.assertRaisesRegex(ValueError, "reappeared"):
                rd.verify_case(row)
            Path(row["decoded"]).unlink()
            Path(row["encoded"]).write_bytes(b"bad")
            with self.assertRaisesRegex(ValueError, "encoded"):
                rd.verify_case(row)

    def test_encoder_ablation_arms_are_separate_pixel_groups(self):
        # Effort 7.5: below the production effort trellis, as these equalities assume.
        base = dict(quality=40.0, color="xyb", rdoq_lambda=0, entropy="runlevel",
                    stripe_block_rows=8, experiment="encoder-ablation",
                    dc_step=[0, 0], dering=True, dc_nearest=True, effort=7.5)
        self.assertNotEqual(rd.pixel_key(base), rd.pixel_key({**base, "dering": False}))
        self.assertNotEqual(rd.pixel_key(base), rd.pixel_key({**base, "dc_step": [64, 64]}))
        self.assertEqual(rd.pixel_key(base), rd.pixel_key({**base, "stripe_block_rows": 1}))
        trellis = {**base, "trellis": "zenjpeg", "trellis_lambda1": 14.5, "trellis_lambda2": 16.5,
                   "trellis_passes": 1, "trellis_min_quality": 0}
        self.assertNotEqual(rd.pixel_key(base), rd.pixel_key(trellis))
        self.assertEqual(rd.pixel_key(base), rd.pixel_key({**base, "trellis": "off"}))
        self.assertNotEqual(rd.pixel_key(trellis), rd.pixel_key({**trellis, "trellis_lambda1": 15.0}))
        self.assertNotEqual(rd.pixel_key(trellis), rd.pixel_key({**trellis, "trellis_passes": 2}))
        self.assertEqual(rd.pixel_key(trellis), rd.pixel_key({**trellis, "trellis_thorough": False}))

    def test_active_effort_trellis_separates_effort_and_stripe_groups(self):
        # Production XYB, run-level AC, full 8x8 layouts, RDOQ off: the effort trellis runs
        # from effort 9 and its pixels depend on the exact effort and the stripe geometry.
        base = dict(quality=40.0, color="xyb", rdoq_lambda=0, entropy="runlevel",
                    stripe_block_rows=8, experiment="encoder-ablation",
                    dc_step=[0, 0], dering=True, dc_nearest=True, effort=7.5)
        for effort in (9.0, 9.9, 10.0):
            active = {**base, "effort": effort}
            self.assertNotEqual(rd.pixel_key(active),
                                rd.pixel_key({**active, "trellis": "off"}))
            self.assertNotEqual(rd.pixel_key(active),
                                rd.pixel_key({**active, "stripe_block_rows": 1}))
        self.assertNotEqual(rd.pixel_key({**base, "effort": 9.0}),
                            rd.pixel_key({**base, "effort": 9.9}))

    def test_inactive_trellis_keeps_effort_and_stripe_groups_together(self):
        base = dict(quality=40.0, color="xyb", rdoq_lambda=0, entropy="runlevel",
                    stripe_block_rows=8, experiment="encoder-ablation",
                    dc_step=[0, 0], dering=True, dc_nearest=True, effort=7.5)
        same = rd.pixel_key(base)
        for stripe in (1, 8):
            # Below effort 9.
            for effort in (0.0, 5.0, 7.5, 8.99):
                self.assertEqual(same, rd.pixel_key(
                    {**base, "effort": effort, "stripe_block_rows": stripe}))
            for effort in (0.0, 7.5, 8.99, 9.0, 9.9, 10.0):
                # Every YCoCg effort.
                ycocg = {**base, "color": "ycocg"}
                self.assertEqual(rd.pixel_key(ycocg), rd.pixel_key(
                    {**ycocg, "effort": effort, "stripe_block_rows": stripe}))
                # Split-Huffman production coding (the effort trellis prices run-level only).
                self.assertEqual(same, rd.pixel_key(
                    {**base, "entropy": "splithuf", "effort": effort,
                     "stripe_block_rows": stripe}))
                # Explicit trellis off.
                self.assertEqual(same, rd.pixel_key(
                    {**base, "trellis": "off", "effort": effort,
                     "stripe_block_rows": stripe}))

    def test_interrupt_prunes_only_cells_with_a_case_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            done = root / "cells" / "done"
            partial = root / "cells" / "partial"
            for cell in (done, partial):
                cell.mkdir(parents=True)
                (cell / "decoded.rgb8").write_bytes(b"px")
            (done / "case.json").write_text("{}")
            self.assertEqual(rd.prune_unrecorded_decoded(root), 1)
            self.assertFalse((done / "decoded.rgb8").exists())
            # Without case.json the pixels' hash is not recorded anywhere yet.
            self.assertTrue((partial / "decoded.rgb8").exists())

    def test_interrupt_archives_partial_scores_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "scores" / "whole").mkdir(parents=True)
            (root / "scores" / "whole" / "scores.json").write_text("{}")
            (root / "scores" / "partial").mkdir()
            (root / "scores" / "partial" / "map.f32").write_bytes(b"x")
            archived = rd.archive_partial_scores(root)
            self.assertEqual([Path(a["original"]).name for a in archived], ["partial"])
            self.assertTrue((root / "scores" / "whole" / "scores.json").exists())
            self.assertFalse((root / "scores" / "partial").exists())

    def test_config_ids_preserve_independent_experiment_axes(self):
        grid = rd.configs([0, 1, 7.5, 100], [1, 25])
        self.assertEqual(len(grid), 58)
        self.assertEqual(len(set(map(rd.fingerprint, grid))), 58)
        self.assertTrue(any(c.get("rdoq_lambda") == 0.08 for c in grid))
        self.assertTrue(any(c["codec"] == "jxl" and c["quality"] == 25 for c in grid))

    def test_retry_preserves_failed_attempts_and_leaves_successes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "records").mkdir()
            (root / "scores").mkdir()
            for status in ("complete", "unsupported", "failed"):
                rd.save(
                    root / "records" / (status + ".json"),
                    dict(status=status, error="exact failure"),
                )
            failed = (root / "records/failed.json").read_bytes()
            partial = root / "scores/partial"
            partial.mkdir()
            (partial / "map.f32").write_bytes(b"partial-map")
            complete = root / "scores/complete"
            complete.mkdir()
            rd.save(complete / "scores.json", dict(score=7))
            before = (complete / "scores.json").read_bytes()
            rd.preserve_failed_attempts(root)
            self.assertFalse((root / "records/failed.json").exists())
            self.assertFalse(partial.exists())
            archived = list((root / "attempts").glob("failed.failed-record.*.json"))
            self.assertEqual(len(archived), 1)
            self.assertEqual(archived[0].read_bytes(), failed)
            maps = list((root / "attempts").glob("partial.partial-score.*/map.f32"))
            self.assertEqual(maps[0].read_bytes(), b"partial-map")
            self.assertEqual((complete / "scores.json").read_bytes(), before)
            for status in ("complete", "unsupported"):
                self.assertTrue((root / "records" / (status + ".json")).exists())
            self.assertEqual(
                len(
                    json.loads(
                        next((root / "attempts").glob("retry-*.json")).read_text()
                    )
                ),
                2,
            )

    def test_fresh_run_requires_matching_binary_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            corpus = root / "corpus.json"
            rd.save(corpus, dict(images=[]))
            binary = root / "binary"
            binary.write_bytes(b"old executable")
            args = SimpleNamespace(
                output=root / "study",
                corpus=corpus,
                images=None,
                qualities="30",
                distances="1",
                codecs=None,
                maps=False,
                config_grid=None,
                keep_decoded=False,
                retry_failed=False,
                binary=binary,
                build_provenance=None,
            )
            with self.assertRaisesRegex(ValueError, "require.*build-provenance"):
                rd.run(args)
            self.assertFalse((args.output / "provenance.json").exists())
            producer = root / "producer.json"
            rd.save(producer, dict(binary_sha256="wrong"))
            args.build_provenance = producer
            with self.assertRaisesRegex(ValueError, "binary differs"):
                rd.run(args)
            self.assertFalse((args.output / "imij-cmp").exists())

    def test_build_routing_accepts_custom_target_and_rejects_stale_native(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            target = root / "custom-target"
            native = target / "release/imij-cmp"
            native.parent.mkdir(parents=True)
            native.write_bytes(b"stale native executable")
            log = root / "build.log"
            log.write_text(
                json.dumps(
                    dict(
                        reason="compiler-artifact",
                        target=dict(name="imij-cmp"),
                        executable=str(native),
                    )
                )
                + "\n"
            )
            self.assertEqual(
                freeze_build.build_executable(log, target), native.resolve()
            )
            cross = target / "aarch64-unknown-linux-gnu/release/imij-cmp"
            log.write_text(
                json.dumps(
                    dict(
                        reason="compiler-artifact",
                        target=dict(name="imij-cmp"),
                        executable=str(cross),
                    )
                )
                + "\n"
            )
            with self.assertRaisesRegex(ValueError, "cross-target"):
                freeze_build.build_executable(log, target)
            log.write_text("run-heavy: no compiler artifact\n")
            with self.assertRaisesRegex(ValueError, "did not emit"):
                freeze_build.build_executable(log, target)


if __name__ == "__main__":
    unittest.main()
