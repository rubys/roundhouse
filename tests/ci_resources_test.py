"""Resource measurements must not hide failures or mislabel machine metrics."""

import csv
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci-resources.py"
SPEC = importlib.util.spec_from_file_location("ci_resources", SCRIPT)
resources = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(resources)


class ResourcesTests(unittest.TestCase):
    def test_cpu_deltas_separate_busy_idle_and_iowait(self):
        before = [100, 10, 20, 400, 50, 2, 3, 4]
        after = [120, 15, 30, 455, 57, 3, 4, 5]
        # 100 ticks: 38 busy, 55 idle, 7 waiting on I/O.
        self.assertEqual(resources.cpu_percent(before, after), (38.0, 7.0))
        self.assertEqual(resources.cpu_percent(before, before), (0.0, 0.0))

    def test_cpu_times_do_not_double_count_guest_ticks(self):
        with patch.object(Path, "read_text", return_value="cpu 1 2 3 4 5 6 7 8 90 91\ncpu0 0\n"):
            self.assertEqual(resources.cpu_times(), [1, 2, 3, 4, 5, 6, 7, 8])

    def test_sample_uses_available_ram_and_unprivileged_disk_space(self):
        disk = os.statvfs_result((4096, 1024, 1000, 300, 250, 0, 0, 0, 0, 255))
        with patch.object(resources, "cpu_times", return_value=[0] * 8), \
             patch.object(Path, "read_text", return_value="MemTotal: 1234 kB\nMemFree: 17 kB\nMemAvailable: 678 kB\n"), \
             patch.object(os, "statvfs", return_value=disk):
            row, _ = resources.sample(resources.time.monotonic(), [0] * 8)
        self.assertEqual(row["machine_mem_total_bytes"], 1234 * 1024)
        self.assertEqual(row["machine_mem_available_bytes"], 678 * 1024)
        self.assertEqual(row["disk_used_bytes"], 700 * 1024)
        self.assertEqual(row["disk_available_bytes"], 250 * 1024)

    def test_intermediate_pressure_is_retained_when_the_command_recovers(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "phase"
            samples = [
                {"elapsed_s": t, "machine_mem_available_bytes": ram,
                 "disk_available_bytes": disk, "disk_used_bytes": 100 - disk}
                for t, ram, disk in [(0, 70, 50), (5, 30, 10), (6, 60, 40)]
            ]
            with patch.object(sys, "argv", [str(SCRIPT), "--out", str(out), "--", "probe"]), \
                 patch.object(resources, "sample", side_effect=[(r, [0] * 8) for r in samples]), \
                 patch.object(subprocess, "Popen") as popen, \
                 patch.object(subprocess, "check_output", return_value="4096\ttarget\n"), \
                 patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": ""}), \
                 patch.object(sys, "stdout", io.StringIO()):
                popen.return_value.wait.side_effect = [subprocess.TimeoutExpired("probe", 5), 0, 0]
                self.assertEqual(resources.main(), 0)
            report = json.loads(out.with_suffix(".json").read_text())
            self.assertEqual(report["machine_min_available_ram_bytes"], 30)
            self.assertEqual(report["disk_min_available_bytes"], 10)
            self.assertEqual(report["disk_used_start_bytes"], 50)
            self.assertEqual(report["disk_used_end_bytes"], 60)
            self.assertEqual(report["wall_s"], 6)
            with out.with_suffix(".csv").open() as stream:
                self.assertEqual(len(list(csv.DictReader(stream))), 3)

    def test_command_output_exit_and_failure_reports_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = os.environ | {"GITHUB_STEP_SUMMARY": str(root / "summary")}
            for code, expected in [("print('child-output')", 0), ("raise SystemExit(37)", 37),
                                   ("import os, signal; os.kill(os.getpid(), signal.SIGTERM)", 143)]:
                out = root / f"phase-{expected}"
                result = subprocess.run(
                    [sys.executable, "-B", str(SCRIPT), "--out", str(out), "--",
                     sys.executable, "-c", code],
                    cwd=root, env=env, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, expected, result.stderr)
                report = json.loads(out.with_suffix(".json").read_text())
                self.assertEqual(report["exit_code"], expected)
                self.assertGreater(report["child_max_single_process_rss_bytes"], 0)
                self.assertEqual(report["cargo_artifacts_bytes"], {})
                self.assertNotIn(code, out.with_suffix(".json").read_text())
                with out.with_suffix(".csv").open() as stream:
                    rows = list(csv.DictReader(stream))
                self.assertEqual(len(rows), 2)  # Initial and final, even a short failure.
                self.assertGreaterEqual(float(rows[-1]["elapsed_s"]), float(rows[0]["elapsed_s"]))
                if expected == 0:
                    self.assertIn("child-output", result.stdout)
            self.assertIn("largest individual child process", (root / "summary").read_text())

    def test_reporting_errors_neither_override_results_nor_abandon_children(self):
        for failure in ["setup", "du", "summary"]:
            for expected in [0, 37]:
                with self.subTest(failure=failure, exit=expected), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    (root / "target/debug/deps").mkdir(parents=True)
                    out = root / "phase"
                    env = os.environ | {"GITHUB_STEP_SUMMARY": str(root / "summary")}
                    if failure == "setup":
                        (root / "blocked").write_text("not a directory")
                        out = root / "blocked/phase"
                    elif failure == "du":
                        du = root / "du"
                        du.write_text("#!/bin/sh\nexit 9\n")
                        du.chmod(0o755)
                        env["PATH"] = f"{root}:{env['PATH']}"
                    else:
                        env["GITHUB_STEP_SUMMARY"] = str(root / "missing/summary")
                    result = subprocess.run(
                        [sys.executable, "-B", str(SCRIPT), "--out", str(out), "--",
                         sys.executable, "-c", "import time; time.sleep(0.03); "
                         f"print('child-completed'); raise SystemExit({expected})"],
                        cwd=root, env=env, capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertIn("child-completed", result.stdout)
                    self.assertIn("Resource measurement unavailable", result.stderr)


if __name__ == "__main__":
    unittest.main()
