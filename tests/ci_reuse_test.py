"""Adversarial receipt tests, also executed by workflow_yaml_parses.rs."""

import copy
import importlib.util
import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
import warnings
import zipfile
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_reuse", Path(__file__).resolve().parents[1] / "scripts/ci-reuse.py"
)
reuse = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reuse)


def bundle(receipt):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("receipt.json", json.dumps(receipt))
    return buffer.getvalue()


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.current = {
            "schema": 1,
            "repository": "rubys/roundhouse",
            "pull_request": 321,
            "head_repository": "contributor/roundhouse",
            "branch": "fix",
            "workflow_id": 17,
            "job": "store-check",
            "fingerprint": "current-inputs",
            "run_id": 500,
        }
        self.receipt = dict(
            self.current,
            run_id=400,
            attempt=1,
            executed=True,
            outcomes=["success", "success"],
        )
        self.run = {
            "id": 400,
            "event": "pull_request",
            "workflow_id": 17,
            "head_branch": "fix",
            "head_repository": {"full_name": "contributor/roundhouse"},
            "run_attempt": 2,
            "conclusion": "failure",
            "pull_requests": [],
        }
        self.job = {
            "name": "store-check",
            "status": "completed",
            "conclusion": "success",
            "html_url": "https://github.com/rubys/roundhouse/actions/runs/400/job/42",
            "steps": [
                {"name": "cargo build", "conclusion": "success"},
                {
                    "name": "The analyzer reports no errors, no warnings and no ingest gaps on the store",
                    "conclusion": "success",
                },
            ],
        }
        self.artifact = {
            "id": 77,
            "name": "ci-executed-store-check-1",
            "expired": False,
        }
        self.paths = []

        class API:
            def get(inner, path):
                self.paths.append(path)
                return {"workflow_runs": [self.run]}

            def pages(inner, path, key):
                self.paths.append(path)
                return iter([self.artifact] if key == "artifacts" else [self.job])

            def bundle(inner, artifact):
                return bundle(self.receipt)

        self.api = API()

    def find(self):
        return reuse.find_execution(self.api, self.current, "store-check")

    def test_accepts_successful_job_of_failed_or_cancelled_workflow_at_exact_attempt(
        self,
    ):
        for conclusion in ("failure", "cancelled", "success", None):
            with self.subTest(conclusion=conclusion):
                self.run["conclusion"] = conclusion
                self.assertEqual(self.find(), self.job["html_url"])
                self.assertIn("actions/runs/400/attempts/1/jobs", self.paths)

    def test_rejects_different_pr_repo_branch_workflow_and_inputs(self):
        for key, value in (
            ("schema", 2),
            ("repository", "elsewhere/roundhouse"),
            ("pull_request", 322),
            ("head_repository", "other/roundhouse"),
            ("branch", "another"),
            ("workflow_id", 18),
            ("job", "compare (rust)"),
            ("fingerprint", "old-inputs"),
        ):
            with self.subTest(key=key), patch.dict(self.receipt, {key: value}):
                self.assertIsNone(self.find())

    def test_rejects_foreign_runs_even_with_a_matching_receipt(self):
        for field, value in (
            ("id", 501),
            ("event", "push"),
            ("workflow_id", 19),
            ("head_branch", "elsewhere"),
            ("head_repository", {"full_name": "other/roundhouse"}),
        ):
            with self.subTest(field=field), patch.dict(self.run, {field: value}):
                self.assertIsNone(self.find())

    def test_does_not_accept_failed_skipped_cancelled_neutral_or_incomplete_jobs(self):
        for conclusion in ("failure", "skipped", "cancelled", "neutral", None):
            with (
                self.subTest(conclusion=conclusion),
                patch.dict(self.job, {"conclusion": conclusion}),
            ):
                self.assertIsNone(self.find())
        with patch.dict(self.job, {"status": "in_progress"}):
            self.assertIsNone(self.find())

    def test_rejects_masked_failure_missing_or_skipped_validation_and_reused_success(
        self,
    ):
        for index in (0, 1):
            for conclusion in ("failure", "skipped", "cancelled", None):
                with (
                    self.subTest(index=index, conclusion=conclusion),
                    patch.dict(self.job["steps"][index], {"conclusion": conclusion}),
                ):
                    self.assertIsNone(self.find())
        with patch.dict(self.receipt, {"outcomes": ["failure", "success"]}):
            self.assertIsNone(self.find())
        with patch.dict(self.receipt, {"executed": False}):
            self.assertIsNone(self.find())
        with patch.dict(self.job, {"steps": self.job["steps"][:1]}):
            self.assertIsNone(self.find())

    def test_rejects_ambiguous_job_or_validation_step(self):
        with patch.object(
            self.api,
            "pages",
            side_effect=lambda path, key: iter(
                [self.artifact]
                if key == "artifacts"
                else [self.job, copy.deepcopy(self.job)]
            ),
        ):
            self.assertIsNone(self.find())
        with patch.dict(
            self.job, {"steps": self.job["steps"] + [self.job["steps"][0]]}
        ):
            self.assertIsNone(self.find())

    def test_expired_invalid_receipt_or_wrong_attempt_is_a_miss(self):
        with patch.dict(self.artifact, {"expired": True}):
            self.assertIsNone(self.find())
        for field, value in (
            ("attempt", 0),
            ("attempt", 3),
            ("attempt", True),
            ("run_id", 399),
            ("outcomes", []),
        ):
            with (
                self.subTest(field=field, value=value),
                patch.dict(self.receipt, {field: value}),
            ):
                self.assertIsNone(self.find())
        with patch.object(self.api, "bundle", return_value=b"not a zip"):
            self.assertIsNone(self.find())

    def test_api_error_is_not_a_hit(self):
        with (
            patch.object(self.api, "get", side_effect=PermissionError),
            self.assertRaises(PermissionError),
        ):
            self.find()

    def test_jobs_and_artifacts_paginate_instead_of_assuming_thirty_jobs(self):
        api = reuse.GitHub("rubys/roundhouse")
        pages = [{"jobs": [{"name": "other"}] * 100}, {"jobs": [self.job]}]
        with patch.object(api, "get", side_effect=pages) as get:
            found = list(api.pages("actions/runs/400/attempts/1/jobs", "jobs"))
            self.assertEqual(len(found), 101)
            self.assertEqual(found[-1], self.job)
            self.assertEqual(
                get.call_args.args[0],
                "actions/runs/400/attempts/1/jobs?per_page=100&page=2",
            )
        with (
            patch.object(api, "get", return_value={"jobs": [{}] * 100}),
            self.assertRaises(ValueError),
        ):
            list(api.pages("jobs", "jobs"))

    def test_zip_paths_duplicates_unexpected_files_and_symlinks_are_rejected(self):
        for extra in ("../receipt.json", "/tmp/receipt.json", "run.sh", "receipt.json"):
            with self.subTest(extra=extra):
                buffer = io.BytesIO()
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore", UserWarning)
                    with zipfile.ZipFile(buffer, "w") as archive:
                        archive.writestr("receipt.json", json.dumps(self.receipt))
                        archive.writestr(extra, b"untrusted")
                with self.assertRaises(ValueError):
                    reuse.read_bundle(buffer.getvalue())
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            link = zipfile.ZipInfo("receipt.json")
            link.external_attr = 0o120777 << 16
            archive.writestr(link, "target")
        with self.assertRaises(ValueError):
            reuse.read_bundle(buffer.getvalue())
        with patch.object(reuse, "MAX_BUNDLE", 10), self.assertRaises(ValueError):
            reuse.read_bundle(bundle(self.receipt))


class InputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.cwd)
        self.git("init", "-q")
        self.git("config", "user.name", "CI test")
        self.git("config", "user.email", "ci@example.invalid")
        # Not left on: `git commit` detaches `git maintenance run --auto`, which can still be writing .git/objects when the temporary directory is removed.
        self.git("config", "maintenance.auto", "false")
        self.git("config", "gc.auto", "0")
        for name in (
            "src/analyze.rs",
            "runtime/ruby/helper.rb",
            "Cargo.lock",
            ".github/workflows/ci.yml",
            "tests/unrelated.rs",
            "tests/support/shared.rs",
            "runtime/ruby/README.md",
            "unknown.file",
        ):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        self.commit()

    def git(self, *args):
        return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL)

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-qm", "test input")

    def test_base_only_merge_change_unknown_addition_deletion_mode_and_shared_inputs_invalidate(
        self,
    ):
        original = self.git("rev-parse", "HEAD").decode().strip()
        for name in (
            "src/analyze.rs",
            "runtime/ruby/helper.rb",
            "Cargo.lock",
            ".github/workflows/ci.yml",
            "tests/support/shared.rs",
            "runtime/ruby/README.md",
        ):
            with self.subTest(name=name):
                self.git("reset", "--hard", original)
                before = reuse.repository_inputs("store-check")
                Path(name).write_text("base-only relevant change")
                self.commit()
                self.assertNotEqual(
                    reuse.repository_inputs("store-check"), before
                )
        self.git("reset", "--hard", original)
        before = reuse.repository_inputs("store-check")
        Path("new-unknown.file").write_text("new input")
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)
        self.git("reset", "--hard", original)
        Path("unknown.file").unlink()
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)
        self.git("reset", "--hard", original)
        Path("unknown.file").chmod(0o755)
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)

    def test_actual_external_contents_modes_paths_and_symlinks_are_not_normalized(self):
        source = self.root / "source"
        source.mkdir()
        file = source / "schema.rb"
        file.write_text("version: 20261002")
        before = reuse.tree_digest(source)
        os.utime(file, (100, 100))
        self.assertEqual(
            reuse.tree_digest(source),
            before,
            "tar/file timestamps are not source bytes",
        )
        file.write_text("version: 20261003")
        self.assertNotEqual(reuse.tree_digest(source), before)
        file.write_text("version: 20261002")
        file.chmod(0o755)
        self.assertNotEqual(reuse.tree_digest(source), before)
        (source / "external").symlink_to(self.root / "unknown.file")
        with self.assertRaises(ValueError):
            reuse.tree_digest(source)
        with self.assertRaises(ValueError):
            reuse.tree_digest(self.root / "missing")

    def test_action_download_markers_are_metadata_but_action_source_is_not(self):
        actions = self.root / "_actions"
        action = actions / "actions/checkout/v5"
        action.mkdir(parents=True)
        source = action / "action.js"
        source.write_text("same action code")
        marker = action.with_suffix(".completed")
        marker.write_text("2026-10-02 10:00:00")
        before = reuse.tree_digest(actions, action_code=True)
        source_before = reuse.tree_digest(actions)
        marker.write_text("2026-10-02 11:00:00")
        self.assertEqual(reuse.tree_digest(actions, action_code=True), before)
        self.assertNotEqual(reuse.tree_digest(actions), source_before)
        source.write_text("new action code")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        source.write_text("same action code")
        source.chmod(0o755)
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        source.chmod(0o644)
        (action / "source.completed").write_text("actual action input")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        (action / "source.completed").unlink()
        (actions / "actions/checkout/unknown.completed").write_text("unknown layout")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)

    def test_observed_environment_changes_invalidate_and_secrets_are_not_persisted(
        self,
    ):
        actions = self.root / "_actions"
        actions.mkdir()
        (actions / "action.js").write_text("resolved action code")
        env = {
            "ImageOS": "ubuntu24",
            "ImageVersion": "20261001.1",
            "RUNNER_OS": "Linux",
            "RUNNER_ARCH": "X64",
            "RUNNER_WORKSPACE": str(self.root / "repo"),
            "CARGO_REGISTRIES_PRIVATE_TOKEN": "must-not-be-recorded",
            "GH_TOKEN": "also-private",
        }
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "command", return_value=b"actual version"),
        ):
            before = reuse.environment_inputs()
            self.assertNotIn("must-not-be-recorded", json.dumps(before))
            self.assertNotIn("also-private", json.dumps(before))
            for name, value in (
                ("ImageVersion", "20261002.2"),
                ("RUSTFLAGS", "-C opt-level=1"),
            ):
                with patch.dict(os.environ, {name: value}):
                    self.assertNotEqual(reuse.environment_inputs(), before)
            (actions / "action.js").write_text("moving action tag changed")
            self.assertNotEqual(reuse.environment_inputs(), before)
            with patch.object(reuse, "command", return_value=b"new Rust toolchain"):
                self.assertNotEqual(
                    reuse.environment_inputs()["rustc"], before["rustc"]
                )

    def test_main_never_looks_up_receipts(self):
        env = {
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_OUTPUT": str(self.root / "outputs"),
            "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
        }
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "GitHub") as api,
        ):
            reuse.probe("store-check", "unused")
            api.assert_not_called()
        self.assertEqual(
            (self.root / "outputs").read_text(), "hit=false\neligible=false\n"
        )

    def test_every_profile_executes_on_main_and_non_pr_events_without_preparation_or_lookup(
        self,
    ):
        for event, ref in (
            ("push", "refs/heads/main"),
            ("workflow_dispatch", "refs/heads/main"),
            ("push", "refs/heads/feature"),
            ("workflow_dispatch", "refs/pull/321/merge"),
            ("pull_request_target", "refs/heads/main"),
            ("pull_request", "refs/heads/main"),
        ):
            for job in reuse.JOBS:
                with self.subTest(event=event, ref=ref, job=job):
                    outputs = self.root / "outputs"
                    outputs.write_text("")
                    with (
                        patch.dict(
                            os.environ,
                            {
                                "GITHUB_EVENT_NAME": event,
                                "GITHUB_REF": ref,
                                "GITHUB_OUTPUT": str(outputs),
                                "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
                            },
                            clear=True,
                        ),
                        patch.object(reuse, "GitHub") as api,
                        patch.object(reuse, "prepare_archive") as prepare,
                        patch.object(reuse, "state_dir") as state,
                    ):
                        reuse.probe(job, "must-not-read-old-or-current-inputs")
                        api.assert_not_called()
                        prepare.assert_not_called()
                        state.assert_not_called()
                    self.assertEqual(outputs.read_text(), "hit=false\neligible=false\n")

    def pr_env(self):
        event = self.root / "event.json"
        event.write_text(
            json.dumps(
                {
                    "number": 321,
                    "pull_request": {
                        "head": {
                            "ref": "fix",
                            "repo": {"full_name": "contributor/roundhouse"},
                        }
                    },
                }
            )
        )
        source = self.root / "source"
        source.mkdir()
        (source / "app.rb").write_text("class Product; end")
        return {
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_REPOSITORY": "rubys/roundhouse",
            "GITHUB_RUN_ID": "500",
            "GITHUB_RUN_ATTEMPT": "1",
            "RUNNER_TEMP": str(self.root / "temp"),
            "GITHUB_OUTPUT": str(self.root / "outputs"),
            "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
        }

    def test_probe_hit_does_not_mint_execution_evidence(self):
        with (
            patch.dict(os.environ, self.pr_env(), clear=True),
            patch.object(reuse, "GitHub") as api,
            patch.object(
                reuse,
                "environment_inputs",
                return_value={"rustc": "observed toolchain"},
            ),
            patch.object(
                reuse,
                "find_execution",
                return_value="https://github.com/original/job/42",
            ),
        ):
            api.return_value.get.return_value = {"workflow_id": 17}
            reuse.probe("store-check", self.root / "source")
            self.assertIn("hit=true\n", (self.root / "outputs").read_text())
            self.assertFalse(
                (reuse.state_dir("store-check") / "bundle").exists()
            )
            self.assertIn("original/job/42", (self.root / "summary").read_text())

    def test_manual_rerun_executes_without_lookup_and_records_only_successful_outcomes(
        self,
    ):
        env = self.pr_env()
        env["GITHUB_RUN_ATTEMPT"] = "2"
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "GitHub") as api,
            patch.object(
                reuse,
                "environment_inputs",
                return_value={"rustc": "observed toolchain"},
            ),
            patch.object(reuse, "find_execution") as lookup,
        ):
            api.return_value.get.return_value = {"workflow_id": 17}
            reuse.probe("store-check", self.root / "source")
            lookup.assert_not_called()
            self.assertNotIn("hit=true", (self.root / "outputs").read_text())
            with self.assertRaises(ValueError):
                reuse.record("store-check", ["success", "skipped"])
            reuse.record("store-check", ["success", "success"])
            receipt = json.loads(
                (reuse.state_dir("store-check") / "bundle/receipt.json").read_text()
            )
            self.assertEqual(receipt["attempt"], 2)
            self.assertIs(receipt["executed"], True)
            self.assertEqual(receipt["outcomes"], ["success", "success"])

    def test_probe_permission_error_exits_nonzero_with_no_hit(self):
        with (
            patch.dict(os.environ, self.pr_env(), clear=True),
            patch.object(reuse, "GitHub") as api,
            patch(
                "sys.argv",
                ["ci-reuse.py", "probe", "--job", "store-check", "--input", "source"],
            ),
        ):
            api.return_value.get.side_effect = PermissionError("denied")
            self.assertEqual(reuse.main(), 1)
            self.assertEqual(
                (self.root / "outputs").read_text(), "hit=false\neligible=false\n"
            )

    def consumer_setup(self):
        project = self.root / "consumer"
        project.mkdir()
        (project / "src").mkdir()
        (project / "src/lib.rs").write_text("fn current() {}")
        (project / "Cargo.toml").write_text('[package]\nname="app"\nversion="0.1.0"\n')
        (project / "Cargo.lock").write_text(
            'version=4\n[[package]]\nname="app"\nversion="0.1.0"\n'
        )
        for base in (project, self.root / "tests/browser_smoke"):
            modules = base / "node_modules"
            modules.mkdir(parents=True)
            (modules / "tool.js").write_text("actual tool")
        browsers = self.root / "browsers"
        browsers.mkdir()
        (browsers / "chromium").write_text("actual browser binary")
        env = {
            "CI": "true",
            "CARGO_HOME": str(self.root / "cargo-home"),
            "PLAYWRIGHT_BROWSERS_PATH": str(browsers),
        }
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, env, clear=True).start()
        patch.object(
            reuse, "environment_inputs", side_effect=lambda: {"rustc": "actual rust"}
        ).start()
        patch.object(reuse, "repository_inputs", return_value="consumer policy").start()
        patch.object(
            reuse, "command", return_value=b"actual tool/package versions"
        ).start()
        return project

    def test_consumer_policy_ignores_producer_edits_but_not_its_harness_or_workflow(
        self,
    ):
        for name in (
            "scripts/smoke",
            "tests/framework_tests_rust.rs",
            "tests/browser_smoke/tests/spec.ts",
        ):
            path = Path(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("consumer contract")
        self.commit()
        jobs = ("smoke-rust", "rust-inflector", "browser-smoke-typescript")
        before = {job: reuse.repository_inputs(job) for job in jobs}
        Path("src/analyze.rs").write_text("compiler edit with identical output")
        self.commit()
        for job in jobs:
            self.assertEqual(reuse.repository_inputs(job), before[job])
        Path("tests/framework_tests_rust.rs").write_text("changed inflector assertion")
        self.commit()
        self.assertNotEqual(
            reuse.repository_inputs("rust-inflector"), before["rust-inflector"]
        )
        self.assertEqual(reuse.repository_inputs("smoke-rust"), before["smoke-rust"])
        Path(".github/workflows/ci.yml").write_text("changed command/environment")
        self.commit()
        for job in jobs:
            self.assertNotEqual(reuse.repository_inputs(job), before[job])
        before = {job: reuse.repository_inputs(job) for job in jobs}
        action = Path(".github/actions/setup-rust/action.yml")
        action.parent.mkdir(parents=True)
        action.write_text("changed local toolchain setup")
        self.commit()
        for job in jobs:
            self.assertNotEqual(reuse.repository_inputs(job), before[job])

    def test_matrix_evidence_is_scoped_to_the_actual_physical_job_and_required_step(
        self,
    ):
        for logical, physical in (
            ("smoke-rust", "smoke (rust)"),
            ("rust-inflector", "compare (rust)"),
        ):
            job = {
                "name": physical,
                "status": "completed",
                "conclusion": "success",
                "steps": [
                    {"name": reuse.JOBS[logical]["checks"][0], "conclusion": "success"}
                ],
            }
            receipt = {"outcomes": ["success"]}
            self.assertIs(reuse.successful_execution([job], receipt, logical), job)
            job["name"] = physical.replace("rust", "swift")
            self.assertIsNone(reuse.successful_execution([job], receipt, logical))
            job["name"] = physical
            job["steps"][0]["conclusion"] = "skipped"
            self.assertIsNone(reuse.successful_execution([job], receipt, logical))

    def test_browser_dist_locks_modules_binary_and_system_packages_invalidate(self):
        project = self.consumer_setup()
        (project / "dist").mkdir()
        asset = project / "dist/worker.js"
        asset.write_text("current compiled worker")
        lock = project / "package-lock.json"
        lock.write_text('{"resolved":"a"}')
        before = reuse.consumer_inputs("browser-smoke-typescript", project)
        for path in (
            asset,
            lock,
            project / "node_modules/tool.js",
            Path("tests/browser_smoke/node_modules/tool.js"),
            self.root / "browsers/chromium",
        ):
            with self.subTest(path=path):
                data = path.read_text()
                path.write_text(data + " changed")
                self.assertNotEqual(
                    reuse.consumer_inputs("browser-smoke-typescript", project), before
                )
                path.write_text(data)
        with patch.object(
            reuse, "command", return_value=b"changed installed package revision"
        ):
            self.assertNotEqual(
                reuse.consumer_inputs("browser-smoke-typescript", project), before
            )
        links = self.root / "browsers/.links"
        links.mkdir()
        (links / "install-location").write_text("different resolver scratch path")
        self.assertEqual(
            reuse.consumer_inputs("browser-smoke-typescript", project), before
        )
        # That exact metadata exception does not extend to generated source.
        (project / ".links").write_text("consumed application input")
        self.assertNotEqual(
            reuse.consumer_inputs("browser-smoke-typescript", project), before
        )

    def test_tools_allow_only_internal_file_symlinks_and_hash_their_targets(self):
        tools = self.root / "tools"
        tools.mkdir()
        (tools / "bin").mkdir()
        target = tools / "cli.js"
        target.write_text("actual cli")
        link = tools / "bin/cli"
        link.symlink_to("../cli.js")
        before = reuse.tree_digest(tools, tool_links=True)
        target.write_text("different cli")
        self.assertNotEqual(reuse.tree_digest(tools, tool_links=True), before)
        for destination in (
            self.root / "unknown.file",
            tools / "bin",
            tools / "missing",
        ):
            link.unlink()
            link.symlink_to(destination)
            with self.assertRaises((ValueError, OSError)):
                reuse.tree_digest(tools, tool_links=True)
        with self.assertRaises(ValueError):
            reuse.tree_digest(tools)
        root_link = self.root / "root-link"
        root_link.symlink_to(tools)
        with self.assertRaises(ValueError):
            reuse.tree_digest(root_link, tool_links=True)

    def test_inflector_witness_includes_lock_and_source_but_not_owned_build_outputs(
        self,
    ):
        project = self.consumer_setup()
        before = reuse.consumer_inputs("rust-inflector", project)
        (project / "target").mkdir()
        (project / "target/binary").write_text("newly compiled output")
        self.assertEqual(reuse.consumer_inputs("rust-inflector", project), before)
        original_lock = (project / "Cargo.lock").read_text()
        (project / "Cargo.lock").write_text(
            original_lock
            + '[[package]]\nname="registry-dep"\nversion="0.2.0"\n'
            + 'source="registry+https://github.com/rust-lang/crates.io-index"\n'
            + 'checksum="actual-resolved-checksum"\n'
        )
        self.assertNotEqual(reuse.consumer_inputs("rust-inflector", project), before)
        (project / "Cargo.lock").write_text(original_lock)
        (project / "src/lib.rs").write_text("fn changed() {}")
        self.assertNotEqual(reuse.consumer_inputs("rust-inflector", project), before)

    def test_cargo_contract_rejects_external_or_excluded_entrypoints_and_dependencies(
        self,
    ):
        project = self.consumer_setup()
        manifest = (project / "Cargo.toml").read_text()
        for extra in (
            '[target."cfg(unix)".dependencies]\nexternal={path="../elsewhere"}\n',
            '[build-dependencies]\nexternal="1"\n',
            '[lib]\npath="target/hidden.rs"\n',
            '[lib]\npath="../external.rs"\n',
            '[[bin]]\npath="target/hidden.rs"\n',
            '[[bin]]\npath="../external.rs"\n',
            '[dependencies]\nexternal={path="../elsewhere"}\n',
        ):
            with self.subTest(extra=extra):
                (project / "Cargo.toml").write_text(manifest + extra)
                with self.assertRaises(ValueError):
                    reuse.consumer_inputs("rust-inflector", project)
        (project / "Cargo.toml").write_text(manifest)
        (project / "Cargo.lock").write_text(
            'version=4\n[[package]]\nname="external-local"\nversion="0.1.0"\n'
        )
        with self.assertRaises(ValueError):
            reuse.consumer_inputs("rust-inflector", project)

    def test_consumer_overrides_and_external_cargo_sources_disable_reuse(self):
        project = self.consumer_setup()
        for flag in (
            "E2E_SKIP",
            "NODE_OPTIONS",
            "SMOKE_MIN_TESTS",
            "SKIP_EMIT",
            "CARGO_REGISTRIES_PRIVATE_TOKEN",
        ):
            with (
                patch.dict(os.environ, {flag: "unwitnessed-or-private"}),
                self.assertRaises(ValueError),
            ):
                reuse.consumer_inputs("rust-inflector", project)
        for flag in ("DATABASE_PATH", "PORT", "DATABASE_POOL_SIZE"):
            for value in ("", "external"):
                with (
                    patch.dict(os.environ, {flag: value}),
                    self.assertRaises(ValueError),
                ):
                    reuse.consumer_inputs("smoke-rust", "archive", project)
        (project / "Cargo.toml").write_text(
            '[dependencies]\nexternal={path="../elsewhere"}\n'
        )
        with self.assertRaises(ValueError):
            reuse.consumer_inputs("rust-inflector", project)

    def test_consumer_recording_refuses_witness_drift_and_reuse_chains(self):
        project = self.consumer_setup()
        env = {
            "RUNNER_TEMP": str(self.root / "temp"),
            "GITHUB_OUTPUT": str(self.root / "outputs"),
        }
        with patch.dict(os.environ, env):
            root = reuse.state_dir("rust-inflector")
            root.mkdir(parents=True)
            state = {
                "inputs": reuse.consumer_inputs("rust-inflector", project),
                "reused": False,
                "local": {"input": str(project)},
            }
            state_path = root / "state.json"
            state_path.write_text(json.dumps(state))
            (project / "src/lib.rs").write_text("changed during test")
            with self.assertRaises(ValueError):
                reuse.record("rust-inflector", ["success"])
            self.assertFalse((root / "bundle").exists())
            state.update(
                inputs=reuse.consumer_inputs("rust-inflector", project), reused=True
            )
            state_path.write_text(json.dumps(state))
            with self.assertRaises(ValueError):
                reuse.record("rust-inflector", ["success"])
            state["reused"] = False
            state_path.write_text(json.dumps(state))
            reuse.record("rust-inflector", ["success"])
            self.assertNotIn(
                "local", json.loads((root / "bundle/receipt.json").read_text())
            )

    def test_source_recording_rejects_external_source_and_environment_drift(self):
        with (
            patch.dict(os.environ, self.pr_env(), clear=True),
            patch.object(
                reuse, "environment_inputs", return_value={"rustc": "before"}
            ) as environment,
        ):
            source = self.root / "source"
            original = (source / "app.rb").read_text()
            job = "store-check"
            root = reuse.state_dir(job)
            root.mkdir(parents=True)
            state = {
                "inputs": reuse.execution_inputs(job, source),
                "reused": False,
                "local": {"input": str(source)},
            }
            (root / "state.json").write_text(json.dumps(state))
            outcomes = ["success"] * len(reuse.JOBS[job]["checks"])
            (source / "app.rb").write_text("different validated source")
            with self.assertRaises(ValueError):
                reuse.record(job, outcomes)
            self.assertFalse((root / "bundle").exists())
            (source / "app.rb").write_text(original)
            environment.return_value = {"rustc": "changed during validation"}
            with self.assertRaises(ValueError):
                reuse.record(job, outcomes)
            self.assertFalse((root / "bundle").exists())
            environment.return_value = {"rustc": "before"}
            reuse.record(job, outcomes)
            self.assertTrue((root / "bundle/receipt.json").is_file())

    def test_archive_resolution_never_prepares_the_validation_extraction_and_rejects_changed_readme(
        self,
    ):
        source = self.root / "rust"
        (source / "e2e").mkdir(parents=True)
        readme = source / "README.md"
        readme.write_text(
            os.environ.get(
                "ROUNDHOUSE_TEST_RUST_README",
                "## Build\n```sh\ncargo build --release\n```\n## Setup\n```sh\nsqlite3 storage/development.sqlite3 < db/seed.sql\n```\n## Test\n```sh\ncargo test\n```\n## End-to-end\n```sh\ncd e2e\nnpm install\nnpx playwright install chromium\nnpx playwright test\n```\n",
            )
        )
        (source / "e2e/package.json").write_text(
            json.dumps(
                {
                    "scripts": {"test": "playwright test"},
                    "devDependencies": {"@playwright/test": "1.59.0"},
                }
            )
        )
        archive = self.root / "rust.tgz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(source, arcname="rust")
        validation = self.root / "validation"
        validation.mkdir()
        with tarfile.open(archive) as tar:
            tar.extractall(validation, filter="data")

        def resolve(*args, **kwargs):
            if args[0] == "tar":
                with tarfile.open(args[2]) as tar:
                    tar.extractall(args[-1], filter="data")
            elif Path(args[0]).name == "smoke":
                # Exercise the real canonical parser, not a Python imitation.
                with subprocess.Popen(args, stdout=subprocess.PIPE) as process:
                    result, _ = process.communicate()
                    self.assertEqual(process.returncode, 0)
                    return result
            return b""

        with (
            patch.object(reuse, "command", side_effect=resolve) as command,
            patch.object(reuse.subprocess, "run") as run,
        ):
            resolved = reuse.prepare_archive(archive, self.root / "resolver")
            self.assertEqual(resolved, self.root / "resolver/rust")
            self.assertEqual(command.call_args.args[:2], ("cargo", "generate-lockfile"))
            self.assertTrue(
                all(
                    call.kwargs["cwd"] == resolved / "e2e"
                    for call in run.call_args_list
                )
            )
            self.assertFalse((validation / "rust/Cargo.lock").exists())
            self.assertFalse((validation / "rust/e2e/node_modules").exists())
        readme.write_text(readme.read_text().replace("npm install\n", ""))
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(source, arcname="rust")
        with self.assertRaises(ValueError):
            reuse.prepare_archive(archive, self.root / "broken-resolver")


if __name__ == "__main__":
    unittest.main()
