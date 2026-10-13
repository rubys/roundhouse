"""ci-npm must retry transient npm install/ci failures."""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-npm"


def write_executable(path: Path, body: str) -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class CiNpmTests(unittest.TestCase):
    def harness(self, npm_body: str) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        shutil.copy(SCRIPT, root / "ci-npm")
        write_executable(root / "npm", npm_body)
        write_executable(
            root / "sleep",
            '#!/bin/sh\necho sleep:"$1" >> "$LOG"\n',
        )
        return root

    def run_npm(self, root: Path, *args: str) -> subprocess.CompletedProcess[str]:
        log = root / "npm.log"
        env = os.environ.copy()
        env["PATH"] = f"{root}:{env['PATH']}"
        env["LOG"] = str(log)
        env["NPM_INSTALL_ATTEMPTS"] = "3"
        env["NPM_INSTALL_RETRY_SLEEP"] = "1"
        result = subprocess.run(
            ["bash", str(root / "ci-npm"), *args],
            check=False,
            text=True,
            capture_output=True,
            env=env,
            cwd=ROOT,
        )
        result.log = log.read_text() if log.exists() else ""  # type: ignore[attr-defined]
        return result

    def test_succeeds_on_first_try(self):
        root = self.harness(
            '#!/bin/sh\necho "npm $*" >> "$LOG"; exit 0\n',
        )
        result = self.run_npm(root, "ci", "--no-audit")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(getattr(result, "log"), "npm ci --no-audit\n")
        self.assertNotIn("sleep:", getattr(result, "log"))

    def test_retries_then_succeeds(self):
        root = self.harness(
            """#!/bin/sh
echo "npm $*" >> "$LOG"
count_file="$LOG.count"
n=$(cat "$count_file" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$count_file"
[ "$n" -ge 2 ] && exit 0
exit 1
""",
        )
        result = self.run_npm(root, "install", "--no-fund")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            getattr(result, "log").count("npm install --no-fund"),
            2,
        )
        self.assertIn("sleep:1", getattr(result, "log"))

    def test_exhausts_attempts(self):
        root = self.harness(
            '#!/bin/sh\necho "npm $*" >> "$LOG"; exit 42\n',
        )
        result = self.run_npm(root, "ci")
        self.assertEqual(result.returncode, 42)
        self.assertEqual(getattr(result, "log").count("npm ci"), 3)
        combined = f"{result.stdout}\n{result.stderr}"
        self.assertIn("failed after 3 attempts", combined)


if __name__ == "__main__":
    unittest.main()
