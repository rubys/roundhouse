"""Bounded unit batches must cover every target and free only finished tests."""

from __future__ import annotations

import importlib.util
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


if __name__ == "__main__":
    unittest.main()
