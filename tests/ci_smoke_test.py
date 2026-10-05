"""Archive smoke recognizes runner counts without weakening execution floors."""

import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

SMOKE = Path(__file__).resolve().parents[1] / "scripts/smoke"


class SmokeCounts(unittest.TestCase):
    def test_node_reporters_count_passes_not_total_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "tests.log"
            for prefix, passed in [("#", 19), ("ℹ", 19), ("ℹ", 0)]:
                with self.subTest(prefix=prefix, passed=passed):
                    log.write_text(f"{prefix} tests 35\n{prefix} pass {passed}\n{prefix} fail 16\n")
                    result = subprocess.run(
                        ["bash", str(SMOKE), "--explain-count", str(log)],
                        capture_output=True, text=True, check=True,
                    )
                    self.assertEqual(result.stdout, f"count_from_log: {passed}\n")
            log.write_text("application says pass 21\n")
            result = subprocess.run(
                ["bash", str(SMOKE), "--explain-count", str(log)],
                capture_output=True, text=True, check=True,
            )
            self.assertEqual(result.stdout, "count_from_log: \n")

    def test_spec_reporter_preserves_the_archive_floor_and_block_failures(self):
        env = os.environ.copy()
        env.pop("SMOKE_MIN_TESTS", None)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "typescript"
            app.mkdir()
            archive = root / "typescript.tgz"
            for passed, exit_command, success in [
                (0, "true", False),
                (20, "true", False),
                (21, "true", True),
                (21, "false", False),
            ]:
                with self.subTest(passed=passed, exit_command=exit_command):
                    (app / "README.md").write_text(
                        "## Test\n```sh\n"
                        f"printf '%s\\n' 'ℹ tests 27' 'ℹ pass {passed}'\n"
                        f"{exit_command}\n```\n"
                    )
                    with tarfile.open(archive, "w:gz") as tar:
                        tar.add(app, arcname="typescript")
                    result = subprocess.run(
                        ["bash", str(SMOKE), "--tgz", str(archive), "typescript"],
                        capture_output=True, text=True, env=env,
                    )
                    self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
                    if success:
                        self.assertIn("executed 21 tests (floor 21)", result.stdout)
                    elif exit_command == "true":
                        self.assertIn(f"executed {passed} tests, below the floor of 21", result.stdout)


if __name__ == "__main__":
    unittest.main()
