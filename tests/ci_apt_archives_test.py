"""ci-apt-archives seeds and harvests .deb files without touching dpkg state."""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-apt-archives"


def write_executable(path: Path, body: str) -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class AptArchivesTests(unittest.TestCase):
    def harness(self) -> tuple[Path, Path, Path, Path]:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        script = root / "ci-apt-archives"
        shutil.copy(SCRIPT, script)
        script.chmod(script.stat().st_mode | stat.S_IXUSR)
        cache = root / "cache"
        archives = root / "archives"
        keep_conf = root / "99-roundhouse-keep-debs"
        archives.mkdir()
        (archives / "partial").mkdir()
        write_executable(
            root / "sudo",
            """#!/bin/bash
printf '%s\\n' "$*" >> "$LOG"
exec "$@"
""",
        )
        write_executable(
            root / "id",
            """#!/bin/bash
case "$1" in
  -u) echo 1000 ;;
  -g) echo 1000 ;;
  *) exit 1 ;;
esac
""",
        )
        return root, cache, archives, keep_conf

    def run_archives(self, root: Path, archives: Path, keep_conf: Path, *args: str):
        log = root / "apt.log"
        env = os.environ.copy()
        env["PATH"] = f"{root}:{env['PATH']}"
        env["LOG"] = str(log)
        env["CI_APT_ARCHIVES_DIR"] = str(archives)
        env["CI_APT_KEEP_CONF"] = str(keep_conf)
        result = subprocess.run(
            ["bash", str(root / "ci-apt-archives"), *args],
            check=False,
            text=True,
            capture_output=True,
            env=env,
        )
        result.log = log.read_text() if log.exists() else ""  # type: ignore[attr-defined]
        return result

    def test_prepare_seeds_debs_and_keeps_downloads(self):
        root, cache, archives, keep_conf = self.harness()
        cache.mkdir()
        (cache / "libvips-dev_1.deb").write_bytes(b"deb")
        result = self.run_archives(root, archives, keep_conf, "prepare", str(cache))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertTrue((archives / "libvips-dev_1.deb").exists())
        self.assertIn("seeded 1", result.stdout)
        self.assertTrue(keep_conf.exists())
        self.assertIn("Keep-Downloaded-Packages", keep_conf.read_text())
        self.assertIn("tee", getattr(result, "log"))

    def test_harvest_copies_archives_into_cache_dir(self):
        root, cache, archives, keep_conf = self.harness()
        (archives / "libjemalloc-dev_1.deb").write_bytes(b"deb")
        result = self.run_archives(root, archives, keep_conf, "harvest", str(cache))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertTrue((cache / "libjemalloc-dev_1.deb").exists())
        self.assertIn("stored 1", result.stdout)

    def test_prepare_with_empty_cache_is_a_noop_seed(self):
        root, cache, archives, keep_conf = self.harness()
        cache.mkdir()
        result = self.run_archives(root, archives, keep_conf, "prepare", str(cache))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("no .deb files to seed", result.stdout)
        self.assertTrue(keep_conf.exists())


if __name__ == "__main__":
    unittest.main()
