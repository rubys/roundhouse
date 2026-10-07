"""Tests for bounded CI archive evidence."""

import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-archive-evidence.py"
SPEC = importlib.util.spec_from_file_location("ci_archive_evidence", SCRIPT)
evidence = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evidence)
SHA = "1" * 40
SPINEL = "2" * 40


class ArchiveEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.env = {
            "GITHUB_SHA": SHA,
            "GITHUB_RUN_ID": "123",
            "GITHUB_RUN_ATTEMPT": "1",
        }

    def tearDown(self):
        self.temp.cleanup()

    def run_cli(self, *args, ok=True, extra_env=None):
        env = os.environ | self.env | (extra_env or {})
        result = subprocess.run(
            ["python3", str(SCRIPT), *map(str, args)],
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        if ok and result.returncode:
            self.fail(result.stderr)
        return result

    def make_inventory(self, group, files, outcome="success", attempt=1, revision=None):
        site = self.root / f"site-{group}-{attempt}"
        for name, contents in files.items():
            path = site / group / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
        rev = self.root / "revision.txt"
        if revision:
            rev.write_text(revision)
        out = self.root / "evidence" / f"inventory-{group}-{attempt}.json"
        with patch.dict(self.env, {"GITHUB_RUN_ATTEMPT": str(attempt)}):
            self.run_cli(
                "inventory",
                "--group",
                group,
                "--root",
                site,
                "--out",
                out,
                "--outcome",
                outcome,
                *(["--spinel-revision-file", rev] if revision else []),
                extra_env={"GITHUB_RUN_ATTEMPT": str(attempt)},
            )
        return out

    def make_validation(
        self, check, data=None, outcome="success", attempt=1, revision=None
    ):
        archive = self.root / f"input-{check}-{attempt}.tgz"
        if data is not None:
            archive.write_bytes(data)
        rev = self.root / "consumer-revision.txt"
        if revision:
            rev.write_text(revision)
        out = self.root / "evidence" / f"validation-{check}-{attempt}.json"
        self.run_cli(
            "validation",
            "--archive",
            archive,
            "--path",
            evidence.CHECKS[check],
            "--check",
            check,
            "--outcome",
            outcome,
            "--out",
            out,
            *(["--spinel-revision-file", rev] if revision else []),
            extra_env={"GITHUB_RUN_ATTEMPT": str(attempt)},
        )
        return out

    def make_report(self, plan, needs=None, ok=True):
        out = self.root / "report.json"
        result = self.run_cli(
            "report",
            "--evidence-dir",
            self.root / "evidence",
            "--out",
            out,
            ok=ok,
            extra_env={
                "CI_PLAN": json.dumps(plan),
                "CI_NEEDS": json.dumps(needs or {}),
            },
        )
        return (json.loads(out.read_text()) if result.returncode == 0 else None), result

    def status(self, report, path):
        return next(item for item in report["archives"] if item["path"] == path)

    def test_inventory_hashes_bytes_independently_and_does_not_mutate(self):
        original = b"archive bytes"
        out = self.make_inventory("browse", {"rust.tgz": original, "map.json": b"{}"})
        item = json.loads(out.read_text())
        rust = next(a for a in item["archives"] if a["path"] == "browse/rust.tgz")
        self.assertEqual(rust["sha256"], hashlib.sha256(original).hexdigest())
        self.assertEqual(
            (self.root / "site-browse-1/browse/rust.tgz").read_bytes(), original
        )
        self.assertIsNone(rust["spinel_revision"])

    def test_same_name_changed_bytes_cannot_pass(self):
        self.make_inventory("browse", {"rust.tgz": b"producer"})
        self.make_validation("smoke-rust", b"changed")
        report, _ = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "failed")

    def test_missing_file_validation_has_null_hash_and_never_passes(self):
        witness = self.make_validation("smoke-rust", None, outcome="success")
        self.assertIsNone(json.loads(witness.read_text())["sha256"])
        self.make_inventory("browse", {"rust.tgz": b"bytes"})
        report, _ = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "failed")

    def test_success_reuse_and_non_success_outcomes(self):
        for outcome, expected in (
            ("success", "passed"),
            ("reused", "reused"),
            ("failure", "failed"),
            ("skipped", "unverified"),
            ("cancelled", "unverified"),
            ("unknown", "unverified"),
        ):
            with self.subTest(outcome=outcome):
                (self.root / "evidence").mkdir(exist_ok=True)
                for path in (self.root / "evidence").iterdir():
                    path.unlink()
                self.make_inventory("browse", {"rust.tgz": b"same"})
                self.make_validation("smoke-rust", b"same", outcome=outcome)
                report, _ = self.make_report(
                    {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
                )
                self.assertEqual(
                    self.status(report, "browse/rust.tgz")["status"], expected
                )

    def test_missing_or_unavailable_selected_witness_is_unverified(self):
        self.make_inventory("browse", {"rust.tgz": b"same"})
        plan = {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        report, _ = self.make_report(plan)
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "unverified")
        self.make_validation("smoke-rust", b"same")
        report, _ = self.make_report(plan, {"smoke": {"result": "cancelled"}})
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "unverified")

    def test_native_spinel_requires_consumer_revision_but_source_archive_does_not_package_one(
        self,
    ):
        self.make_inventory("browse", {"spinel.tgz": b"native"}, revision=SPINEL)
        self.make_validation("spinel-smoke", b"native")
        plan = {"archives": ["spinel"], "jobs": ["spinel-smoke"], "smoke": []}
        report, _ = self.make_report(plan)
        item = self.status(report, "browse/spinel.tgz")
        self.assertEqual(item["status"], "unverified")
        inventory = json.loads(
            (self.root / "evidence/inventory-browse-1.json").read_text()
        )
        self.assertIsNone(inventory["archives"][0]["spinel_revision"])

    def test_native_and_docker_spinel_revision_rules_are_distinct(self):
        data = b"campfire"
        self.make_inventory(
            "campfire", {"spinel.tgz": data, "docker.tgz": data}, revision=SPINEL
        )
        self.make_validation("campfire-smoke", data, revision=SPINEL)
        self.make_validation("campfire-smoke-docker", data)
        plan = {
            "archives": [],
            "jobs": [
                "campfire-archive-build",
                "campfire-smoke",
                "campfire-smoke-docker",
            ],
            "smoke": [],
        }
        report, _ = self.make_report(plan)
        self.assertEqual(self.status(report, "campfire/spinel.tgz")["status"], "passed")
        docker = self.status(report, "campfire/docker.tgz")
        self.assertEqual(docker["status"], "passed")
        self.assertEqual(docker["spinel_revision"], SPINEL)

    def test_docker_unknown_packaging_revision_is_unverified(self):
        self.make_inventory("campfire", {"docker.tgz": b"docker"})
        self.make_validation("campfire-smoke-docker", b"docker")
        plan = {
            "archives": [],
            "jobs": ["campfire-archive-build", "campfire-smoke-docker"],
            "smoke": [],
        }
        report, _ = self.make_report(plan)
        self.assertEqual(
            self.status(report, "campfire/docker.tgz")["status"], "unverified"
        )

    def test_non_tgz_inventory_is_not_certified_by_tgz_smoke(self):
        self.make_inventory(
            "browse", {"rust.tgz": b"x", "rust.zip": b"x", "rust.json": b"x"}
        )
        self.make_validation("smoke-rust", b"x")
        report, _ = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "passed")
        self.assertEqual(
            self.status(report, "browse/rust.zip")["status"], "not-selected"
        )
        self.assertEqual(
            self.status(report, "browse/rust.json")["status"], "not-selected"
        )

    def test_latest_attempt_is_selected_deterministically(self):
        self.make_inventory("browse", {"rust.tgz": b"old"}, attempt=1)
        self.make_validation("smoke-rust", b"old", attempt=1)
        self.make_inventory("browse", {"rust.tgz": b"new"}, attempt=2)
        self.make_validation("smoke-rust", b"new", attempt=2)
        self.env["GITHUB_RUN_ATTEMPT"] = "2"
        report, _ = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        self.assertEqual(
            self.status(report, "browse/rust.tgz")["sha256"],
            hashlib.sha256(b"new").hexdigest(),
        )
        self.assertEqual(report["producers"][0]["attempt"], 2)

    def test_wrong_source_or_run_and_conflicts_are_errors(self):
        path = self.make_inventory("browse", {"rust.tgz": b"x"})
        item = json.loads(path.read_text())
        for field, value in (("source_sha", "3" * 40), ("run_id", "999")):
            with self.subTest(field=field):
                changed = dict(item, **{field: value})
                path.write_text(json.dumps(changed))
                _, result = self.make_report(
                    {"archives": [], "jobs": [], "smoke": []}, ok=False
                )
                self.assertEqual(result.returncode, 2)
                path.write_text(json.dumps(item))
        duplicate = self.root / "evidence/duplicate.json"
        duplicate.write_text(json.dumps(dict(item, producer_outcome="failure")))
        _, result = self.make_report(
            {"archives": [], "jobs": [], "smoke": []}, ok=False
        )
        self.assertEqual(result.returncode, 2)

    def test_missing_selected_archive_is_reported(self):
        self.make_inventory("browse", {})
        report, _ = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        item = self.status(report, "browse/rust.tgz")
        self.assertEqual(item["status"], "unverified")
        self.assertIsNone(item["sha256"])

    def test_producer_failure_after_partial_output_cannot_validate(self):
        self.make_inventory("browse", {"rust.tgz": b"same"}, outcome="failure")
        self.make_validation("smoke-rust", b"same")
        report, result = self.make_report(
            {"archives": ["rust"], "jobs": [], "smoke": ["rust"]}
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "failed")

    def test_unselected_existing_witness_and_failed_matrix_sibling(self):
        self.make_inventory("browse", {"go.tgz": b"go", "rust.tgz": b"rust"})
        self.make_validation("smoke-go", b"go")
        self.make_validation("smoke-rust", b"rust")
        report, _ = self.make_report(
            {"archives": ["go", "rust"], "jobs": ["smoke"], "smoke": ["go"]},
            {"build-site": {"result": "success"}, "smoke": {"result": "failure"}},
        )
        self.assertEqual(self.status(report, "browse/go.tgz")["status"], "passed")
        self.assertEqual(
            self.status(report, "browse/rust.tgz")["status"], "not-selected"
        )

    def test_future_evidence_attempt_is_rejected(self):
        self.make_inventory("browse", {"rust.tgz": b"future"}, attempt=2)
        _, result = self.make_report(
            {"archives": ["rust"], "smoke": ["rust"]}, ok=False
        )
        self.assertEqual(result.returncode, 2)

    def test_old_success_does_not_certify_failed_or_masked_rerun(self):
        self.make_inventory("browse", {"spinel.tgz": b"native"})
        self.make_validation("spinel-smoke", b"native", revision=SPINEL)
        self.env["GITHUB_RUN_ATTEMPT"] = "2"
        for result in ["failure", "success"]:
            with self.subTest(result=result):
                report, _ = self.make_report(
                    {"archives": ["spinel"], "jobs": ["spinel-smoke"]},
                    {
                        "build-site": {"result": "success"},
                        "spinel-smoke": {
                            "result": result,
                            "outputs": {"execution": "failure"},
                        },
                    },
                )
                item = self.status(report, "browse/spinel.tgz")
                self.assertEqual(item["status"], "unverified")
                self.assertEqual(item["validation_attempt"], 1)

    def test_publication_checks_bytes_not_validation_success(self):
        self.make_inventory(
            "browse", {"rust.tgz": b"repro", "rust.zip": b"zip"}, outcome="failure"
        )
        report, _ = self.make_report({"archives": ["rust"], "smoke": ["rust"]})
        self.assertEqual(self.status(report, "browse/rust.tgz")["status"], "failed")
        site = self.root / "site-browse-1"
        args = ("verify", "--root", site, "--report", self.root / "report.json")
        self.run_cli(*args)
        self.assertEqual((site / "browse/rust.tgz").read_bytes(), b"repro")
        document = json.loads((self.root / "report.json").read_text())
        self.assertTrue(self.status(document, "browse/rust.tgz")["published"])
        (site / "browse/rust.tgz").unlink()
        self.run_cli(*args)
        document = json.loads((self.root / "report.json").read_text())
        self.assertFalse(self.status(document, "browse/rust.tgz")["published"])
        self.assertEqual(self.status(document, "browse/rust.tgz")["status"], "failed")
        (site / "browse/rust.zip").write_bytes(b"changed")
        self.assertEqual(self.run_cli(*args, ok=False).returncode, 2)
        (site / "browse/rust.zip").write_bytes(b"zip")
        (site / "browse/unknown.tgz").write_bytes(b"new")
        self.assertEqual(self.run_cli(*args, ok=False).returncode, 2)
        (site / "browse/unknown.tgz").unlink()
        self.assertEqual(
            self.run_cli(
                *args, ok=False, extra_env={"GITHUB_SHA": "3" * 40}
            ).returncode,
            2,
        )

    def test_cli_rejects_mismatched_path_check_and_missing_identity(self):
        result = self.run_cli(
            "validation",
            "--archive",
            self.root / "none",
            "--path",
            "browse/spinel.tgz",
            "--check",
            "smoke-rust",
            "--outcome",
            "success",
            "--out",
            self.root / "out.json",
            ok=False,
        )
        self.assertEqual(result.returncode, 2)
        result = self.run_cli(
            "inventory",
            "--group",
            "browse",
            "--root",
            self.root,
            "--out",
            self.root / "out.json",
            "--outcome",
            "success",
            ok=False,
            extra_env={"GITHUB_SHA": "short"},
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
