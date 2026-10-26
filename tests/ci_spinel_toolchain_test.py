"""Producer/consumer contracts for the spinel_toolchain CI archive."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci-spinel-toolchain.py"
SPEC = importlib.util.spec_from_file_location("ci_spinel_toolchain", SCRIPT)
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class SpinelToolchainInventoryTests(unittest.TestCase):
    def inventory(self, **overrides):
        data = {
            "version": ci.FORMAT_VERSION,
            "source_sha": "a" * 40,
            "workspace": "/home/runner/work/roundhouse/roundhouse",
            "target": "/home/runner/work/roundhouse/roundhouse/target",
            "archive_sha256": "b" * 64,
            "spinel_toolchain": {
                "path": "target/debug/deps/spinel_toolchain-deadbeef",
                "sha256": "c" * 64,
            },
            "roundhouse": {"path": "target/debug/roundhouse", "sha256": "d" * 64},
            "tiny_api": {
                "path": "target/debug/deps/tiny_api-feedface",
                "sha256": "e" * 64,
            },
            "tests": ["real_blog_spinel_tests_pass", "identical_cli_emission_reuses_native_build_but_changed_ruby_rebuilds"],
        }
        data.update(overrides)
        return data

    def test_valid_inventory_is_accepted(self):
        self.assertEqual(ci.validate_inventory(self.inventory())["tests"][0], "real_blog_spinel_tests_pass")

    def test_absolute_or_parent_binary_paths_are_rejected(self):
        for path in ("/tmp/spinel_toolchain", "target/../evil", "../roundhouse"):
            with self.subTest(path=path):
                bad = self.inventory()
                bad["spinel_toolchain"]["path"] = path
                with self.assertRaises(ci.ContractError):
                    ci.validate_inventory(bad)

    def test_duplicate_or_empty_tests_fail(self):
        with self.assertRaises(ci.ContractError):
            ci.validate_inventory(self.inventory(tests=["a", "a"]))
        with self.assertRaises(ci.ContractError):
            ci.validate_inventory(self.inventory(tests=[]))

    def test_restore_rejects_workspace_mismatch(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive = root / "bundle.tar.gz"
            inventory_path = root / "inventory.json"
            binary = root / "target/debug/deps/spinel_toolchain-1"
            roundhouse = root / "target/debug/roundhouse"
            tiny = root / "target/debug/deps/tiny_api-1"
            for path, body in (
                (binary, b"spinel"),
                (roundhouse, b"rh"),
                (tiny, b"tiny"),
            ):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(body)
                path.chmod(path.stat().st_mode | stat.S_IXUSR)
            with tarfile.open(archive, "w:gz") as tar:
                for path in (binary, roundhouse, tiny):
                    tar.add(path, arcname=path.relative_to(root).as_posix())
            inventory = {
                "version": ci.FORMAT_VERSION,
                "source_sha": "a" * 40,
                "workspace": "/other/workspace",
                "target": str(root / "target"),
                "archive_sha256": sha256(archive.read_bytes()),
                "spinel_toolchain": {
                    "path": binary.relative_to(root).as_posix(),
                    "sha256": sha256(b"spinel"),
                },
                "roundhouse": {
                    "path": roundhouse.relative_to(root).as_posix(),
                    "sha256": sha256(b"rh"),
                },
                "tiny_api": {
                    "path": tiny.relative_to(root).as_posix(),
                    "sha256": sha256(b"tiny"),
                },
                "tests": ["only_test"],
            }
            inventory_path.write_text(json.dumps(inventory))
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=None)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="a" * 40
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                with self.assertRaises(ci.ContractError) as caught:
                    ci.restore(args)
            self.assertIn("workspace differs", str(caught.exception))


class SpinelToolchainRestoreTests(unittest.TestCase):
    def _bundle(self, root: Path, *, source_sha: str = "a" * 40):
        archive = root / "bundle.tar.gz"
        inventory_path = root / "inventory.json"
        binary = root / "target/debug/deps/spinel_toolchain-1"
        roundhouse = root / "target/debug/roundhouse"
        tiny = root / "target/debug/deps/tiny_api-1"
        for path, body in ((binary, b"spinel"), (roundhouse, b"rh"), (tiny, b"tiny")):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(body)
            path.chmod(path.stat().st_mode | stat.S_IXUSR)
        with tarfile.open(archive, "w:gz") as tar:
            for path in (binary, roundhouse, tiny):
                tar.add(path, arcname=path.relative_to(root).as_posix())
        inventory = {
            "version": ci.FORMAT_VERSION,
            "source_sha": source_sha,
            "workspace": str(root),
            "target": str(root / "target"),
            "archive_sha256": sha256(archive.read_bytes()),
            "spinel_toolchain": {
                "path": binary.relative_to(root).as_posix(),
                "sha256": sha256(b"spinel"),
            },
            "roundhouse": {
                "path": roundhouse.relative_to(root).as_posix(),
                "sha256": sha256(b"rh"),
            },
            "tiny_api": {
                "path": tiny.relative_to(root).as_posix(),
                "sha256": sha256(b"tiny"),
            },
            "tests": ["only_test"],
        }
        inventory_path.write_text(json.dumps(inventory))
        return archive, inventory_path, inventory

    def test_restore_rejects_archive_digest_mismatch(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive, inventory_path, inventory = self._bundle(root)
            inventory["archive_sha256"] = "f" * 64
            inventory_path.write_text(json.dumps(inventory))
            env_file = root / "bins.env"
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=env_file)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="a" * 40
            ), mock.patch.object(
                ci, "list_ignored_tests", return_value=["only_test"]
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                with self.assertRaises(ci.ContractError) as caught:
                    ci.restore(args)
            self.assertIn("archive digest differs", str(caught.exception))

    def test_restore_rejects_source_sha_mismatch(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive, inventory_path, _ = self._bundle(root, source_sha="a" * 40)
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=None)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="b" * 40
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                with self.assertRaises(ci.ContractError) as caught:
                    ci.restore(args)
            self.assertIn("source SHA differs", str(caught.exception))

    def test_restore_writes_env_file_and_relists_tests(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive, inventory_path, inventory = self._bundle(root)
            # Wipe staged binaries so restore must extract them.
            for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
                (root / inventory[key]["path"]).unlink()
            env_file = root / "bins.env"
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=env_file)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="a" * 40
            ), mock.patch.object(
                ci, "list_ignored_tests", return_value=["only_test"]
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                self.assertEqual(ci.restore(args), 0)
            text = env_file.read_text()
            self.assertIn(f"SPINEL_TOOLCHAIN_BIN={root / inventory['spinel_toolchain']['path']}", text)
            self.assertIn(f"TINY_API_BIN={root / inventory['tiny_api']['path']}", text)
            self.assertIn(f"ROUNDHOUSE_BIN={root / inventory['roundhouse']['path']}", text)
            for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
                path = root / inventory[key]["path"]
                self.assertTrue(path.is_file(), key)
                self.assertTrue(os.access(path, os.X_OK), key)

    def test_restore_rejects_duplicate_archive_members(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive, inventory_path, inventory = self._bundle(root)
            binary = root / inventory["spinel_toolchain"]["path"]
            # Rebuild with the spinel binary listed twice under the same name.
            with tarfile.open(archive, "w:gz") as tar:
                for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
                    path = root / inventory[key]["path"]
                    tar.add(path, arcname=path.relative_to(root).as_posix())
                tar.add(binary, arcname=binary.relative_to(root).as_posix())
            inventory["archive_sha256"] = sha256(archive.read_bytes())
            inventory_path.write_text(json.dumps(inventory))
            for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
                (root / inventory[key]["path"]).unlink()
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=None)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="a" * 40
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                with self.assertRaises(ci.ContractError) as caught:
                    ci.restore(args)
            self.assertIn("archive members differ", str(caught.exception))

    def test_restore_rejects_unexpected_archive_members(self):
        with tempfile.TemporaryDirectory(prefix="spinel-toolchain-ci-") as tmp:
            root = Path(tmp)
            archive = root / "bundle.tar.gz"
            inventory_path = root / "inventory.json"
            binary = root / "target/debug/deps/spinel_toolchain-1"
            roundhouse = root / "target/debug/roundhouse"
            tiny = root / "target/debug/deps/tiny_api-1"
            evil = root / "evil.bin"
            for path, body in (
                (binary, b"spinel"),
                (roundhouse, b"rh"),
                (tiny, b"tiny"),
                (evil, b"nope"),
            ):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(body)
                path.chmod(path.stat().st_mode | stat.S_IXUSR)
            with tarfile.open(archive, "w:gz") as tar:
                for path in (binary, roundhouse, tiny, evil):
                    tar.add(path, arcname=path.relative_to(root).as_posix())
            inventory = {
                "version": ci.FORMAT_VERSION,
                "source_sha": "a" * 40,
                "workspace": str(root),
                "target": str(root / "target"),
                "archive_sha256": sha256(archive.read_bytes()),
                "spinel_toolchain": {
                    "path": binary.relative_to(root).as_posix(),
                    "sha256": sha256(b"spinel"),
                },
                "roundhouse": {
                    "path": roundhouse.relative_to(root).as_posix(),
                    "sha256": sha256(b"rh"),
                },
                "tiny_api": {
                    "path": tiny.relative_to(root).as_posix(),
                    "sha256": sha256(b"tiny"),
                },
                "tests": ["only_test"],
            }
            inventory_path.write_text(json.dumps(inventory))
            for path in (binary, roundhouse, tiny, evil):
                path.unlink()
            args = mock.Mock(archive=archive, inventory=inventory_path, env_file=None)
            with mock.patch.object(ci, "ROOT", root), mock.patch.object(
                ci, "source_sha", return_value="a" * 40
            ), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=False):
                with self.assertRaises(ci.ContractError) as caught:
                    ci.restore(args)
            self.assertIn("archive members differ", str(caught.exception))
            self.assertFalse(evil.exists())


class SpinelToolchainWorkflowTests(unittest.TestCase):
    def test_producer_and_shards_share_one_compile(self):
        workflow = Path(".github/workflows/ci.yml").read_text()
        self.assertIn("spinel-toolchain-build:", workflow)
        self.assertIn("scripts/ci-spinel-toolchain.py build", workflow)
        self.assertIn("scripts/ci-spinel-toolchain.py restore", workflow)
        producer = workflow.split("spinel-toolchain-build:", 1)[1].split("\n  spinel-toolchain:", 1)[0]
        shards = workflow.split("\n  spinel-toolchain:", 1)[1].split("\n  compare:", 1)[0]
        self.assertIn("./.github/actions/setup-rust", producer)
        self.assertNotIn("./.github/actions/setup-rust", shards)
        self.assertNotIn("Swatinem/rust-cache@", shards)
        self.assertNotIn("cargo test --test spinel_toolchain --no-run", shards)
        self.assertIn("spinel-toolchain-build.outputs.artifact-id", shards)
        self.assertIn("run-tests --kind toolchain", shards)
        # Producer is compile-only; shards still wait on generate-fixture.
        self.assertNotIn("real-blog-fixture", producer)
        self.assertNotIn("generate-fixture", producer.split("needs:", 1)[1].split("\n", 1)[0])
        self.assertIn("generate-fixture", shards)


if __name__ == "__main__":
    unittest.main()
