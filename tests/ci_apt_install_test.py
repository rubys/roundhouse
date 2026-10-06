"""ci-apt-install must skip installed packages and only refresh the index on miss."""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-apt-install"


def write_executable(path: Path, body: str) -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class AptInstallTests(unittest.TestCase):
    def harness(self, *, dpkg_body: str, sudo_body: str) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        shutil.copy(SCRIPT, root / "ci-apt-install")
        write_executable(root / "ci-apt-bound", "#!/bin/sh\necho bound >> \"$LOG\"\n")
        write_executable(root / "dpkg-query", dpkg_body)
        write_executable(root / "sudo", sudo_body)
        return root

    def run_install(self, root: Path, *packages: str) -> subprocess.CompletedProcess[str]:
        log = root / "apt.log"
        env = os.environ.copy()
        env["PATH"] = f"{root}:{env['PATH']}"
        env["LOG"] = str(log)
        result = subprocess.run(
            ["bash", str(root / "ci-apt-install"), *packages],
            check=False,
            text=True,
            capture_output=True,
            env=env,
            cwd=ROOT,
        )
        result.log = log.read_text() if log.exists() else ""  # type: ignore[attr-defined]
        return result

    def test_skips_when_every_package_is_already_installed(self):
        root = self.harness(
            dpkg_body="#!/bin/sh\necho installed\n",
            sudo_body="#!/bin/sh\necho unexpected-apt >> \"$LOG\"; exit 99\n",
        )
        result = self.run_install(root, "libvips42", "lld")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already installed", result.stdout)
        self.assertIn("nothing to install", result.stdout)
        self.assertEqual(getattr(result, "log"), "")

    def test_installs_missing_packages_without_update_when_the_index_has_them(self):
        root = self.harness(
            dpkg_body="""#!/bin/sh
pkg=$3
[ "$pkg" = present ] && echo installed && exit 0
echo not-installed
""",
            sudo_body="""#!/bin/sh
printf '%s\\n' "$*" >> "$LOG"
exit 0
""",
        )
        result = self.run_install(root, "present", "missing")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        log = getattr(result, "log")
        self.assertIn("bound", log)
        self.assertIn("apt-get install", log)
        self.assertIn("missing", log)
        self.assertNotIn("present", log)
        self.assertNotIn("apt-get update", log)

    def test_refreshes_the_index_only_after_the_image_index_misses(self):
        root = self.harness(
            dpkg_body="#!/bin/sh\necho not-installed\n",
            sudo_body="""#!/bin/sh
printf '%s\\n' "$*" >> "$LOG"
case " $* " in
  *" apt-get update "*) exit 0 ;;
  *" apt-get install "*)
    if [ -f "$LOG.once" ]; then
      exit 0
    fi
    touch "$LOG.once"
    exit 100
    ;;
esac
exit 0
""",
        )
        result = self.run_install(root, "libclang-dev")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        log = getattr(result, "log")
        self.assertIn("image index missed packages", result.stderr)
        self.assertLess(log.find("apt-get install"), log.find("apt-get update"))
        self.assertGreater(log.rfind("apt-get install"), log.find("apt-get update"))


if __name__ == "__main__":
    unittest.main()
