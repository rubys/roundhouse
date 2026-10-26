"""Site publication provenance, input selection, and retained-byte boundaries."""

import argparse
import copy
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ci_site", ROOT / "scripts/ci-site.py")
site = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(site)
SHA = "1" * 40


def context(source=SHA):
    return {"GITHUB_REPOSITORY": site.REPOSITORY, "GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": source}


def artifact(attempt, **overrides):
    return {"id": attempt, "name": f"github-pages-{attempt}", "expired": False, "digest": "sha256:" + "a" * 64, "size_in_bytes": 100, **overrides}


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.deployment = {"id": 7, "environment": "github-pages", "ref": "main", "sha": SHA}
        self.status = {"state": "success", "log_url": f"https://github.com/{site.REPOSITORY}/actions/runs/100/job/200"}
        self.workflow = {"id": 100, "repository": {"full_name": site.REPOSITORY}, "path": ".github/workflows/full-ci.yml", "head_branch": "main", "head_sha": SHA, "event": "schedule", "conclusion": "failure"}
        self.job = {"id": 200, "run_id": 100, "run_attempt": 2, "status": "completed", "conclusion": "success"}

    def choose(self, artifacts=None):
        return site.trusted_baseline(self.deployment, self.status, self.workflow, self.job, artifacts or [artifact(1), artifact(2), artifact(3)])

    def test_only_manual_canonical_main_can_publish(self):
        self.assertEqual(site.publication_context(context()), SHA)
        for key, value in [("GITHUB_REPOSITORY", "Shopify/roundhouse"), ("GITHUB_REF", "refs/heads/topic"), ("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_SHA", "main")]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                site.publication_context(context() | {key: value})

    def test_uses_successful_deployment_attempt_not_overall_run_or_newer_artifact(self):
        selected = self.choose()
        self.assertEqual(selected["artifact"]["id"], 2)
        self.assertEqual(selected["attempt"], 2)
        self.workflow.update(status="in_progress", conclusion=None, run_attempt=3)
        self.assertEqual(self.choose()["artifact"]["id"], 2)

    def test_rejects_foreign_or_failed_deployment_provenance(self):
        for record, key, value in [
            (self.deployment, "ref", "topic"), (self.deployment, "sha", "2" * 40),
            (self.workflow, "repository", {"full_name": "Shopify/roundhouse"}),
            (self.workflow, "path", ".github/workflows/ci.yml"), (self.workflow, "event", "pull_request"),
            (self.job, "conclusion", "failure"), (self.job, "run_id", 101),
            (self.status, "log_url", "https://github.com/elsewhere/roundhouse/actions/runs/100/job/200"),
        ]:
            before = copy.deepcopy(record)
            with self.subTest(key=key, value=value):
                record[key] = value
                with self.assertRaises(ValueError):
                    self.choose()
                record.clear()
                record.update(before)

    def test_does_not_fall_back_when_deployed_artifact_expired_or_is_ambiguous(self):
        for candidates in [[artifact(1), artifact(2, expired=True)], [artifact(1), artifact(2, digest=None)], [artifact(2), artifact(2, id=99)]]:
            with self.subTest(candidates=candidates), self.assertRaises(ValueError):
                self.choose(candidates)

    def test_latest_deployment_failure_does_not_replace_last_success(self):
        records = [self.deployment | {"id": 8}, self.deployment]
        outer = self
        class API:
            def pages(self, path, key=None):
                return iter([artifact(2)] if key else records)
            def get(self, path):
                if path.startswith("deployments/8/"):
                    return [{"state": "failure"}]
                if path.startswith("deployments/7/"):
                    return [outer.status]
                return outer.job if "/jobs/" in path else outer.workflow
        self.assertEqual(site.latest_baseline(API())["deployment_id"], 7)


class SiteTreeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.checkout = self.base / "checkout"
        self.checkout.mkdir()
        self.git("init", "-q")
        self.put("site/index.html", "<h1>old page</h1>")
        self.put("site/removed.html", "old page to remove")
        self.put("wasm/src/lib.rs", "compiler source")
        self.put("wasm/lib/engine.mjs", "browser module")
        for name in site.SURFACES:
            self.put(f"wasm/{name}/index.html", f"<h1>{name}</h1>")
            self.put(f"wasm/{name}/{name}.js", "browser source")
        self.old = self.commit()
        self.output = self.base / "published"
        self.output.mkdir()
        site.copy_assets(self.checkout, self.output, self.old, site.source_assets(site.git_files(self.checkout, self.old)))
        self.generated = {
            "browse/ruby.tgz": b"retained archive",
            "bench/index.html": b"retained benchmark",
            "campfire/conformance/summary.json": b'{"retained":true}',
            "guide/index.html": b"retained generated guide",
            "lib/fixture.json": b'{"app/models/item.rb":"class Item; end"}',
            "lib/roundhouse_wasm.wasm": b"retained compiler",
            "ide/apps.json": b'{"apps":[]}',
        }
        for path, data in self.generated.items():
            self.published(path, data)
        self.report = {
            "schema": 1, "source_sha": self.old, "run_id": "100", "attempt": 1,
            "archives": [{"path": "browse/ruby.tgz", "sha256": hashlib.sha256(self.generated["browse/ruby.tgz"]).hexdigest(), "size": len(self.generated["browse/ruby.tgz"])}],
        }
        self.published("ci/archive-results.json", json.dumps(self.report).encode())
        self.baseline = {"deployment_id": 7, "source_sha": self.old, "run_id": 100, "attempt": 1, "workflow": ".github/workflows/full-ci.yml"}

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.checkout, stderr=subprocess.DEVNULL).decode().strip()

    def commit(self):
        self.git("add", ".")
        self.git("-c", "user.name=Site Test", "-c", "user.email=site@example.test", "commit", "--no-gpg-sign", "-qm", "source")
        return self.git("rev-parse", "HEAD")

    def put(self, path, data):
        destination = self.checkout / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(data)

    def published(self, path, data):
        destination = self.output / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)

    def plan(self):
        source = self.git("rev-parse", "HEAD")
        return site.plan_update(self.checkout, self.output, self.baseline, source)

    def assemble(self, state, wasm=None):
        path = self.base / "state.json"
        site.write_json(path, state)
        env = os.environ | context(state["source_sha"])
        return subprocess.run(["python3", str(ROOT / "scripts/ci-site.py"), "assemble", "--root", str(self.output), "--state", str(path), "--checkout", str(self.checkout), *(["--wasm", str(wasm)] if wasm else [])], env=env, text=True, capture_output=True)

    def test_static_update_removes_only_owned_files_and_preserves_generated_bytes(self):
        self.put("site/index.html", "<h1>new page</h1>")
        self.put("site/new.html", "new page")
        (self.checkout / "site/removed.html").unlink()
        self.commit()
        state = self.plan()
        self.assertFalse(state["wasm"])
        self.assertEqual(state["browser"], [])
        result = self.assemble(state)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.output / "index.html").read_text(), "<h1>new page</h1>")
        self.assertEqual((self.output / "new.html").read_text(), "new page")
        self.assertFalse((self.output / "removed.html").exists())
        for path, data in self.generated.items():
            self.assertEqual((self.output / path).read_bytes(), data, path)
        self.assertEqual(json.loads((self.output / "ci/archive-results.json").read_text()), self.report)
        metadata = json.loads((self.output / site.METADATA).read_text())
        self.assertEqual(metadata["wasm_source_sha"], self.old)
        self.assertEqual(metadata["archive_source_sha"], self.old)
        self.assertEqual(metadata["source_sha"], state["source_sha"])

    def test_frontend_change_checks_only_its_surface_without_rebuilding_compiler(self):
        self.put("wasm/fractal/fractal.js", "changed browser source")
        self.commit()
        state = self.plan()
        self.assertFalse(state["wasm"])
        self.assertEqual(state["browser"], ["fractal"])

    def test_browser_selection_uses_the_published_bytes_not_only_source_history(self):
        self.published("fractal/fractal.js", b"outdated deployed browser code")
        state = self.plan()
        self.assertFalse(state["wasm"])
        self.assertEqual(state["browser"], ["fractal"])

    def test_shared_browser_change_checks_every_surface_without_compiler_build(self):
        self.put("wasm/lib/engine.mjs", "changed browser module")
        self.commit()
        state = self.plan()
        self.assertFalse(state["wasm"])
        self.assertEqual(state["browser"], list(site.SURFACES))

    def test_compiler_change_requires_a_fresh_build_before_changing_site_files(self):
        self.put("wasm/src/lib.rs", "new compiler source")
        self.put("site/index.html", "new page")
        self.commit()
        state = self.plan()
        self.assertTrue(state["wasm"])
        self.assertEqual(state["browser"], list(site.SURFACES))
        result = self.assemble(state)
        self.assertEqual(result.returncode, 2)
        self.assertEqual((self.output / "index.html").read_text(), "<h1>old page</h1>")

    def test_static_source_cannot_replace_generated_files(self):
        self.put("site/bench/index.html", "not retained")
        self.commit()
        result = self.assemble(self.plan())
        self.assertEqual(result.returncode, 2)
        self.assertEqual((self.output / "bench/index.html").read_bytes(), self.generated["bench/index.html"])

    def test_reused_compiler_and_archive_bytes_are_checked(self):
        self.put("site/index.html", "new page")
        self.commit()
        state = self.plan()
        for path in ["lib/roundhouse_wasm.wasm", "browse/ruby.tgz"]:
            with self.subTest(path=path):
                self.published(path, b"unexpected published bytes")
                result = self.assemble(state)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(json.loads((self.output / "ci/archive-results.json").read_text()), self.report)
                self.published(path, self.generated[path])

    def test_copy_reads_the_pinned_commit_not_uncommitted_source(self):
        self.put("site/index.html", "committed page")
        self.commit()
        state = self.plan()
        self.put("site/index.html", "uncommitted page")
        result = self.assemble(state)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.output / "index.html").read_text(), "committed page")

    def test_second_site_update_keeps_original_compiler_provenance(self):
        self.put("site/index.html", "first update")
        self.commit()
        first = self.plan()
        self.assertEqual(self.assemble(first).returncode, 0)
        self.baseline.update(source_sha=first["source_sha"], workflow=".github/workflows/publish-site.yml")
        self.put("site/index.html", "second update")
        self.commit()
        second = self.plan()
        self.assertFalse(second["wasm"])
        self.assertEqual(second["wasm_source_sha"], self.old)
        self.put("wasm/src/lib.rs", "compiler changed after both site updates")
        self.commit()
        self.assertTrue(self.plan()["wasm"])

    def test_page_artifact_extraction_rejects_path_escape_and_links(self):
        for name, kind in [("../escape", tarfile.REGTYPE), ("/escape", tarfile.REGTYPE), ("link", tarfile.SYMTYPE), ("hardlink", tarfile.LNKTYPE)]:
            with self.subTest(name=name):
                tar_bytes = io.BytesIO()
                with tarfile.open(fileobj=tar_bytes, mode="w") as tar:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = "../escape"
                    tar.addfile(member, io.BytesIO())
                bundle = self.base / "artifact.zip"
                with zipfile.ZipFile(bundle, "w") as archive:
                    archive.writestr("artifact.tar", tar_bytes.getvalue())
                output = self.base / f"extracted-{kind.decode()}"
                with self.assertRaises(ValueError):
                    site.extract_site(bundle, output)
                self.assertFalse((self.base / "escape").exists())
                self.assertEqual((self.output / "index.html").read_text(), "<h1>old page</h1>")


if __name__ == "__main__":
    unittest.main()
