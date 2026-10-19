"""Archive consumers must retain every active case exactly once across shards."""

from __future__ import annotations

import copy
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci-unit-tests.py"
SPEC = importlib.util.spec_from_file_location("ci_unit_tests", SCRIPT)
unit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(unit)


def case(ignored=False):
    return {"kind": "test", "ignored": ignored,
            "filter-match": {"status": "mismatch", "reason": "ignored"} if ignored else {"status": "matches"}}


def inventory():
    return {
        "test-count": 5,
        "rust-suites": {
            "roundhouse": {"binary-id": "roundhouse", "binary-name": "roundhouse", "kind": "lib",
                           "status": "listed", "testcases": {
                               "registry::resolves": case(), "registry::native_only": case(True)}},
            "roundhouse::all_tests": {"binary-id": "roundhouse::all_tests", "binary-name": "all_tests",
                                      "kind": "test", "status": "listed", "testcases": {
                                          "real_blog::zero_errors": case(),
                                          "emit_and_run::ruby_executes": case(),
                                          "spinel_toolchain::native": case(True)}},
        },
    }


class UnitInventoryTests(unittest.TestCase):
    def setUp(self):
        self.data = inventory()
        self.cases = {
            ("roundhouse", "registry::resolves"): False,
            ("roundhouse", "registry::native_only"): True,
            ("roundhouse::all_tests", "real_blog::zero_errors"): False,
            ("roundhouse::all_tests", "emit_and_run::ruby_executes"): False,
            ("roundhouse::all_tests", "spinel_toolchain::native"): True,
        }
        self.active = {identity for identity, ignored in self.cases.items() if not ignored}

    def test_inventory_retains_unit_integration_and_ignored_identities(self):
        identities, selected = unit.parse_inventory(self.data)
        self.assertEqual(identities, self.cases)
        self.assertEqual(selected, self.active)

    def test_disabled_combined_target_cannot_pass_with_library_tests_alone(self):
        self.data["rust-suites"]["roundhouse::all_tests"]["testcases"] = {}
        self.data["test-count"] = 2
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_missing_combined_target_is_rejected(self):
        del self.data["rust-suites"]["roundhouse::all_tests"]
        self.data["test-count"] = 2
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_unlisted_suite_cannot_hide_cases(self):
        self.data["rust-suites"]["roundhouse"]["status"] = "skipped"
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_unexpected_active_filter_is_rejected(self):
        self.data["rust-suites"]["roundhouse"]["testcases"]["registry::resolves"]["filter-match"] = {
            "status": "mismatch", "reason": "default-filter"}
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_ignored_cases_cannot_be_selected(self):
        self.data["rust-suites"]["roundhouse"]["testcases"]["registry::native_only"]["filter-match"] = {
            "status": "matches"}
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_inconsistent_count_and_malformed_ignore_state_fail(self):
        for value in (False, 4, 6):
            data = copy.deepcopy(self.data)
            data["test-count"] = value
            with self.assertRaises(unit.ContractError):
                unit.parse_inventory(data)
        self.data["rust-suites"]["roundhouse"]["testcases"]["registry::resolves"]["ignored"] = 0
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_all_ignored_inventory_cannot_become_a_successful_empty_run(self):
        for suite in self.data["rust-suites"].values():
            for name in suite["testcases"]:
                suite["testcases"][name] = case(True)
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_partition_inventory_keeps_nonselected_and_ignored_cases(self):
        self.data["rust-suites"]["roundhouse"]["testcases"]["registry::resolves"]["filter-match"] = {
            "status": "mismatch", "reason": "partition"}
        cases, selected = unit.parse_inventory(self.data, partitioned=True)
        self.assertEqual(cases, self.cases)
        self.assertEqual(selected, self.active - {("roundhouse", "registry::resolves")})
        with self.assertRaises(unit.ContractError):
            unit.parse_inventory(self.data)

    def test_complete_partitions_exclude_ignored_cases(self):
        partitions = [{("roundhouse", "registry::resolves")},
                      {("roundhouse::all_tests", "real_blog::zero_errors")},
                      {("roundhouse::all_tests", "emit_and_run::ruby_executes")}]
        unit.check_partition_coverage(self.cases, partitions)
        for invalid in (
            partitions[:-1],
            [*partitions, partitions[0]],
            [*partitions, {("roundhouse", "registry::native_only")}],
            [*partitions, {("unknown", "not_in_archive")}],
        ):
            with self.assertRaises(unit.ContractError):
                unit.check_partition_coverage(self.cases, invalid)

    def test_shard_bounds_reject_zero_negative_boolean_and_out_of_range(self):
        for index, count in ((0, 0), (-1, 4), (4, 4), (0, -1), (0, True), (True, 4)):
            with self.assertRaises(unit.ContractError):
                unit.validate_shard(index, count)
        unit.validate_shard(0, 1)
        unit.validate_shard(3, 4)

    def test_real_child_failures_preserve_exit_status(self):
        with self.assertRaises(unit.ChildFailure) as failure:
            unit.check_command([sys.executable, "-c", "raise SystemExit(7)"])
        self.assertEqual(failure.exception.status, 7)
        self.assertEqual(unit.ChildFailure(-9).status, 137)

    def test_inherited_git_locations_cannot_change_archive_provenance(self):
        expected = unit.source_sha()
        with tempfile.TemporaryDirectory() as temporary:
            foreign = Path(temporary)
            git = ["git", "-c", "user.name=Inventory Test",
                   "-c", "user.email=inventory@example.invalid", "-c", "commit.gpgsign=false",
                   "-c", "core.hooksPath=/dev/null", "-c", "maintenance.auto=false", "-c", "gc.auto=0"]
            subprocess.run([*git, "init", "-q"], cwd=foreign, check=True)
            subprocess.run([*git, "commit", "--allow-empty", "-qm", "foreign"], cwd=foreign, check=True)
            with patch.dict(os.environ, {"GIT_DIR": str(foreign / ".git"),
                                         "GIT_COMMON_DIR": str(foreign / ".git"),
                                         "GIT_WORK_TREE": str(foreign)}):
                self.assertEqual(unit.source_sha(), expected)


class UnitManifestTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.workspace = Path(self.directory.name).resolve()
        self.target = self.workspace / "target"
        self.cases, _selected = unit.parse_inventory(inventory())
        self.manifest = {
            "version": 1, "source_sha": "a" * 40, "workspace": str(self.workspace),
            "target": str(self.target), "archive_sha256": "b" * 64, "shard_count": 4,
            "tests": unit.encode_cases(self.cases),
        }
        self.anchors = {"workspace": self.workspace, "target": self.target,
                        "source": "a" * 40, "archive_sha256": "b" * 64, "shard_count": 4}

    def test_valid_manifest_preserves_all_case_states(self):
        self.assertEqual(unit.validate_manifest(self.manifest, **self.anchors), self.cases)

    def test_archive_source_paths_and_shard_count_must_match(self):
        invalid = {
            "workspace": self.workspace / "other-checkout",
            "target": self.workspace / "other-target",
            "source": "c" * 40,
            "archive_sha256": "c" * 64,
            "shard_count": 3,
        }
        for key, value in invalid.items():
            anchors = dict(self.anchors, **{key: value})
            with self.assertRaises(unit.ContractError):
                unit.validate_manifest(self.manifest, **anchors)

    def test_duplicate_identity_is_not_silently_overwritten(self):
        self.manifest["tests"].append(dict(self.manifest["tests"][0]))
        with self.assertRaises(unit.ContractError):
            unit.manifest_cases(self.manifest)

    def test_unknown_versions_fields_and_missing_fields_are_rejected(self):
        for version in (0, 2, True, "1"):
            manifest = dict(self.manifest, version=version)
            with self.assertRaises(unit.ContractError):
                unit.manifest_cases(manifest)
        manifest = dict(self.manifest, extra="unexpected")
        with self.assertRaises(unit.ContractError):
            unit.manifest_cases(manifest)
        del self.manifest["source_sha"]
        with self.assertRaises(unit.ContractError):
            unit.manifest_cases(self.manifest)

    def test_empty_malformed_or_all_ignored_manifest_is_rejected(self):
        for rows in ([], [{"binary": "x", "name": "y", "ignored": 0}],
                     [{"binary": "x", "name": "y", "ignored": True}]):
            manifest = dict(self.manifest, tests=rows)
            with self.assertRaises(unit.ContractError):
                unit.manifest_cases(manifest)
        for key, value in (("source_sha", "not-a-sha"), ("archive_sha256", "z" * 64),
                           ("workspace", "relative/path"), ("shard_count", False)):
            manifest = dict(self.manifest, **{key: value})
            with self.assertRaises(unit.ContractError):
                unit.manifest_cases(manifest)


if __name__ == "__main__":
    unittest.main()
