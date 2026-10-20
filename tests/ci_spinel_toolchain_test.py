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


if __name__ == "__main__":
    unittest.main()
