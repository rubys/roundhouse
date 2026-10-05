"""Bounded unit batches must cover every target and free only finished tests."""

from __future__ import annotations

import importlib.util
import io
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci-unit-tests.py"
SPEC = importlib.util.spec_from_file_location("ci_unit_tests", SCRIPT)
unit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(unit)


def write_executable(path: Path, body: str = "#!/bin/sh\nexit 0\n") -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class UnitBatchTests(unittest.TestCase):
    def test_chunks_cover_every_name_exactly_once(self):
        names = [f"t{i:03d}" for i in range(42)]
        batches = unit.chunks(names, 20)
        self.assertEqual([len(b) for b in batches], [20, 20, 2])
        self.assertEqual([name for batch in batches for name in batch], names)

    def test_owned_paths_include_sidecars_but_not_neighbors(self):
        with tempfile.TemporaryDirectory() as directory:
            deps = Path(directory)
            exe = deps / "cli_check-abc123def4567890"
            sidecar = deps / "cli_check-abc123def4567890.cli_check.x.rcgu.dwo"
            depfile = deps / "cli_check-abc123def4567890.d"
            neighbor = deps / "cli_check-abc123def4567890ee"
            other = deps / "analyze-ffffffffffffffff"
            other_dwo = deps / "analyze-ffffffffffffffff.analyze.y.rcgu.dwo"
            helper = deps / "roundhouse-ffffffabcdef0123"
            for path in (exe, neighbor, other, helper):
                write_executable(path)
            for path in (sidecar, depfile, other_dwo):
                path.write_bytes(b"x" * 8)
            owned = {p.name for p in unit.owned_integration_paths(exe)}
            self.assertEqual(owned, {exe.name, sidecar.name, depfile.name})

    def test_integration_executables_match_hash_stems_only(self):
        with tempfile.TemporaryDirectory() as directory:
            deps = Path(directory)
            keep = deps / "cli_check-abc123def4567890"
            write_executable(keep)
            (deps / "cli_check-abc123def4567890.d").write_text("d")
            write_executable(deps / "cli_check-notahashvalue!!")
            write_executable(deps / "cli_check_extra-abc123def4567890")
            write_executable(deps / "analyze-abc123def4567890")
            found = unit.integration_executables(deps, "cli_check")
            self.assertEqual([p.name for p in found], [keep.name])

    def test_free_removes_only_requested_batch(self):
        with tempfile.TemporaryDirectory() as directory:
            deps = Path(directory)
            keep_exe = deps / "analyze-aaaaaaaaaaaaaaaa"
            free_exe = deps / "cli_check-bbbbbbbbbbbbbbbb"
            free_dwo = deps / "cli_check-bbbbbbbbbbbbbbbb.cli_check.dwo"
            keep_bin = deps / "roundhouse-cccccccccccccccc"
            for path in (keep_exe, free_exe, keep_bin):
                write_executable(path)
            free_dwo.write_bytes(b"dwo")
            freed = unit.free_integration_targets(deps, ["cli_check"])
            self.assertGreater(freed, 0)
            self.assertFalse(free_exe.exists())
            self.assertFalse(free_dwo.exists())
            self.assertTrue(keep_exe.exists())
            self.assertTrue(keep_bin.exists())

    def test_metadata_listing_matches_tests_directory(self):
        names, _target = unit.package_plan(unit.cargo_metadata())
        unit.verify_coverage(names)
        stems = sorted(p.stem for p in Path("tests").glob("*.rs"))
        self.assertEqual(names, stems)

    def test_failure_propagates_and_skips_reclaim(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "debug" / "deps"
            deps.mkdir(parents=True)
            exe = deps / "fails-0123456789abcdef"
            write_executable(exe)
            dwo = deps / "fails-0123456789abcdef.fails.dwo"
            dwo.write_text("dwo")
            calls: list[list[str]] = []

            def fake_run(args, **_kwargs):
                calls.append(list(args))
                if "--test" in args and "--no-run" not in args and "--lib" not in args:
                    return 17
                return 0

            with mock.patch.object(unit, "cargo_metadata", return_value={
                "target_directory": str(root),
                "packages": [{
                    "name": "roundhouse",
                    "targets": [{"name": "fails", "kind": ["test"]}],
                }],
            }), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "run_cargo", side_effect=fake_run
            ):
                code = unit.main(["--batch-size", "1", "--no-timings"])
            self.assertEqual(code, 17)
            self.assertTrue(exe.exists(), "failed batch artifacts must remain")
            self.assertTrue(dwo.exists())
            self.assertTrue(any("--lib" in c and "--bins" in c for c in calls))

    def test_successful_batch_reclaims_integration_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "debug" / "deps"
            deps.mkdir(parents=True)
            test_exe = deps / "ok-0123456789abcdef"
            write_executable(test_exe)
            dwo = deps / "ok-0123456789abcdef.ok.dwo"
            dwo.write_text("dwo")
            helper = deps / "roundhouse-fedcba9876543210"
            write_executable(helper)

            def fake_run(args, **_kwargs):
                return 0

            with mock.patch.object(unit, "cargo_metadata", return_value={
                "target_directory": str(root),
                "packages": [{
                    "name": "roundhouse",
                    "targets": [{"name": "ok", "kind": ["test"]}],
                }],
            }), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "run_cargo", side_effect=fake_run
            ):
                code = unit.main(["--batch-size", "1", "--no-timings"])
            self.assertEqual(code, 0)
            self.assertFalse(test_exe.exists())
            self.assertFalse(dwo.exists())
            self.assertTrue(helper.exists())

    def test_invalid_shard_bounds_do_not_invoke_cargo(self):
        cases = [
            ["--shard-count", "0"],
            ["--shard-count", "-1"],
            ["--shard-index", "-1"],
            ["--shard-index", "1", "--shard-count", "1"],
            ["--shard-index", "3", "--shard-count", "3"],
        ]
        for argv in cases:
            with self.subTest(argv=argv):
                with mock.patch.object(unit, "cargo_metadata") as meta, mock.patch.object(
                    unit, "run_cargo"
                ) as cargo:
                    with self.assertRaises(SystemExit) as ctx:
                        unit.main(argv)
                    self.assertIsInstance(ctx.exception.code, str)
                    self.assertIn("shard", ctx.exception.code)
                    meta.assert_not_called()
                    cargo.assert_not_called()

    def test_select_shard_interleaves_nondivisible_names(self):
        names = [f"t{i:02d}" for i in range(7)]
        expected = {
            0: ["t00", "t03", "t06"],
            1: ["t01", "t04"],
            2: ["t02", "t05"],
        }
        shards = {index: unit.select_shard(names, index, 3) for index in range(3)}
        self.assertEqual(shards, expected)
        union = [name for index in range(3) for name in shards[index]]
        self.assertEqual(sorted(union), names)
        self.assertEqual(len(union), len(set(union)))
        for left in range(3):
            for right in range(left + 1, 3):
                self.assertTrue(set(shards[left]).isdisjoint(shards[right]))
        self.assertEqual(unit.select_shard(names, 0, 1), names)

    def test_list_only_prints_selected_shard_after_global_coverage(self):
        names = [f"t{i:02d}" for i in range(7)]
        expected = ["t01", "t04"]
        meta = {
            "target_directory": "/tmp",
            "packages": [{
                "name": "roundhouse",
                "targets": [{"name": name, "kind": ["test"]} for name in names],
            }],
        }
        stdout = io.StringIO()
        stderr = io.StringIO()
        with mock.patch.object(unit, "cargo_metadata", return_value=meta), mock.patch.object(
            unit, "verify_coverage"
        ) as coverage, mock.patch.object(unit, "run_cargo") as cargo, mock.patch(
            "sys.stdout", stdout
        ), mock.patch("sys.stderr", stderr):
            code = unit.main(["--list-only", "--shard-index", "1", "--shard-count", "3"])
        self.assertEqual(code, 0)
        cargo.assert_not_called()
        coverage.assert_called_once_with(names)
        self.assertEqual(stdout.getvalue().splitlines(), expected)
        err = stderr.getvalue()
        self.assertIn("shard 1 of 3", err)
        self.assertIn("2 selected of 7", err)

    def test_nonzero_shard_skips_library_and_binary_units(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "debug" / "deps"
            deps.mkdir(parents=True)
            alpha = deps / "alpha-aaaaaaaaaaaaaaaa"
            beta = deps / "beta-bbbbbbbbbbbbbbbb"
            gamma = deps / "gamma-cccccccccccccccc"
            for path in (alpha, beta, gamma):
                write_executable(path)
            calls: list[list[str]] = []

            def fake_run(args, **_kwargs):
                calls.append(list(args))
                return 0

            with mock.patch.object(unit, "cargo_metadata", return_value={
                "target_directory": str(root),
                "packages": [{
                    "name": "roundhouse",
                    "targets": [
                        {"name": "alpha", "kind": ["test"]},
                        {"name": "beta", "kind": ["test"]},
                        {"name": "gamma", "kind": ["test"]},
                    ],
                }],
            }), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "run_cargo", side_effect=fake_run
            ):
                code = unit.main(
                    ["--shard-index", "1", "--shard-count", "2", "--no-timings"]
                )
            self.assertEqual(code, 0)
            self.assertFalse(any("--lib" in call or "--bins" in call for call in calls))
            tested = [
                call[call.index("--test") + 1] for call in calls if "--test" in call
            ]
            self.assertTrue(tested)
            self.assertTrue(all(name == "beta" for name in tested))
            self.assertTrue(all("--locked" in call for call in calls))
            self.assertFalse(beta.exists())
            self.assertTrue(alpha.exists())
            self.assertTrue(gamma.exists())

    def test_global_missing_metadata_is_rejected_despite_filtering(self):
        stems = sorted(p.stem for p in Path("tests").glob("*.rs"))
        self.assertGreater(len(stems), 3)
        omitted = stems[0]
        incomplete = [stem for stem in stems if stem != omitted]
        with mock.patch.object(unit, "cargo_metadata", return_value={
            "target_directory": "/tmp",
            "packages": [{
                "name": "roundhouse",
                "targets": [{"name": stem, "kind": ["test"]} for stem in incomplete],
            }],
        }), mock.patch.object(unit, "run_cargo") as cargo:
            with self.assertRaises(SystemExit) as ctx:
                unit.main(["--list-only", "--shard-index", "1", "--shard-count", "3"])
            cargo.assert_not_called()
            message = str(ctx.exception)
            self.assertIn("missing", message)
            self.assertIn(omitted, message)

    def test_sharded_failure_propagates_and_skips_reclaim(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "debug" / "deps"
            deps.mkdir(parents=True)
            keep = deps / "alpha-aaaaaaaaaaaaaaaa"
            fails = deps / "beta-bbbbbbbbbbbbbbbb"
            dwo = deps / "beta-bbbbbbbbbbbbbbbb.beta.dwo"
            write_executable(keep)
            write_executable(fails)
            dwo.write_text("dwo")
            calls: list[list[str]] = []

            def fake_run(args, **_kwargs):
                calls.append(list(args))
                if "--test" in args and "--no-run" not in args:
                    return 17
                return 0

            with mock.patch.object(unit, "cargo_metadata", return_value={
                "target_directory": str(root),
                "packages": [{
                    "name": "roundhouse",
                    "targets": [
                        {"name": "alpha", "kind": ["test"]},
                        {"name": "beta", "kind": ["test"]},
                    ],
                }],
            }), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "run_cargo", side_effect=fake_run
            ):
                code = unit.main(
                    ["--shard-index", "1", "--shard-count", "2", "--no-timings"]
                )
            self.assertEqual(code, 17)
            self.assertFalse(any("--lib" in call for call in calls))
            self.assertTrue(fails.exists(), "failed batch artifacts must remain")
            self.assertTrue(dwo.exists())
            self.assertTrue(keep.exists())


if __name__ == "__main__":
    unittest.main()
