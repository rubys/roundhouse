"""Ruby campfire-suite runs may overlap without changing the serial contract.

Parallelism is acceptable only when each file still sees the serial
launch: the default SECRET_KEY_BASE, combined stdout/stderr order, the
real uid, and a private Active Storage root. A missing or refusing
unshare is a serial run, not a failed suite.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "scripts/campfire-suite"
CAMPFIRE_COMPARE = ROOT / "scripts/campfire-compare"
CAMPFIRE_E2E = ROOT / "scripts/campfire-e2e"

SHIM = textwrap.dedent(
    """\
    class {klass}
      def test_one
      end
    end
    __t = {klass}.new
    begin
      __t.test_one
    end
    """
)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def executable(path: Path, body: str) -> None:
    write(path, body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class CampfireSuiteParallelTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="campfire-suite-opt-")
        self.addCleanup(self.tmp.cleanup)
        self.emit = Path(self.tmp.name) / "emit"
        write(self.emit / "db/seed.sql", "")
        (self.emit / "tmp/storage").mkdir(parents=True)

    def _run(self, jobs: str, *, env_extra=None, unset=()) -> subprocess.CompletedProcess[str]:
        # Unique names: two calls with the same jobs value must not share a file.
        stamp = len(list(Path(self.tmp.name).glob("tally-*.txt")))
        tally = Path(self.tmp.name) / f"tally-{stamp}.txt"
        fail_log = Path(self.tmp.name) / f"fail-{stamp}.txt"
        env = os.environ.copy()
        for key in unset:
            env.pop(key, None)
        if env_extra:
            env.update(env_extra)
        env["SUITE_TEST_RUN_ID"] = str(stamp)
        result = subprocess.run(
            [
                "bash",
                str(SUITE),
                "--reuse",
                str(self.emit),
                "--jobs",
                jobs,
                "--tally",
                str(tally),
                "--fail-log",
                str(fail_log),
            ],
            check=False,
            text=True,
            capture_output=True,
            env=env,
        )
        result.tally_path = tally  # type: ignore[attr-defined]
        result.fail_path = fail_log  # type: ignore[attr-defined]
        return result

    def _note(self, tag: str) -> str:
        return (self.emit / "tmp" / f"note-{tag}.txt").read_text()

    def _require_mounts(self):
        if not shutil.which("unshare") or not shutil.which("setpriv"):
            self.skipTest("unshare or setpriv is not installed")
        source = Path(self.tmp.name) / "probe-source"
        destination = Path(self.tmp.name) / "probe-destination"
        source.mkdir()
        destination.mkdir()
        probe = subprocess.run(
            ["unshare", "--user", "--map-current-user", "--keep-caps", "--mount",
             "mount", "--bind", str(source), str(destination)],
            capture_output=True,
        )
        if probe.returncode:
            self.skipTest("this host refuses unprivileged bind mounts")

    def _two_file_emit(self) -> None:
        write(
            self.emit / "Makefile",
            "SPINEL_TESTS := test/models/alpha_test \\\n"
            "\ttest/models/beta_test \\\n"
            "\ttest/models/noisy_test\n\n",
        )
        write(
            self.emit / "test/models/noisy_test.rb",
            textwrap.dedent(
                """\
                warn "STDERR-BEFORE"
                raise "STDOUT-AFTER"
                __t = Object.new
                def __t.test_noise; end
                begin
                  __t.test_noise
                end
                puts "NoisyTest: 1 tests passed"
                """
            ),
        )
        # Both files write and then read the same storage filename. A
        # shared root would let one file observe the other's bytes.
        for tag, klass in (("alpha", "AlphaTest"), ("beta", "BetaTest")):
            write(
                self.emit / "test/models" / f"{tag}_test.rb",
                SHIM.format(klass=klass)
                + textwrap.dedent(
                    f"""\
                    module ActionController; class Base; end; end
                    require "{ROOT / 'runtime/ruby/rails.rb'}"
                    require "{ROOT / 'runtime/spinel/active_storage_disk.rb'}"
                    Rails.env_name = ENV["RAILS_ENV"]
                    raise "argv0 changed" unless $0 == "test/models/{tag}_test.rb"
                    raise "arguments changed" unless ARGV.empty?
                    raise "cwd changed" unless Dir.pwd == File.expand_path("../..", __dir__)
                    warn "STDERR-FIRST-{tag}"
                    path = "tmp/storage/shared-name"
                    File.write(path, "{tag}\\n")
                    service = ActiveStorage::Service.new
                    service.upload("shared-name", "{tag}\\n")
                    if ENV["OVERLAP_SUITE_TESTS"] == "1"
                      ready = "tmp/ready-" + ENV.fetch("SUITE_TEST_RUN_ID") + "-"
                      File.write(ready + "{tag}", "ready")
                      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 10
                      until File.exist?(ready + "{'beta' if tag == 'alpha' else 'alpha'}")
                        raise "sibling did not overlap" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
                        sleep 0.01
                      end
                    end
                    seen = File.read(path)
                    raise "shared storage leaked: " + seen.inspect unless seen == "{tag}\\n"
                    seen = service.download("shared-name")
                    raise "Active Storage leaked: " + seen.inspect unless seen == "{tag}\\n"
                    puts "STDOUT-SECOND-{tag}"
                    File.write("tmp/note-{tag}.txt", [
                      "uid=#{{Process.uid}}",
                      "euid=#{{Process.euid}}",
                      "secret=#{{ENV.fetch("SECRET_KEY_BASE")}}",
                    ].join("\\n") + "\\n" + File.read("/proc/self/status").lines.grep(/^Cap/).join)
                    puts "{klass}: 1 tests passed"
                    """
                ),
            )

    def test_parallel_isolation_matches_serial_contract(self):
        self._require_mounts()
        self._two_file_emit()
        for rails_env in (None, "test", "production"):
            with self.subTest(rails_env=rails_env):
                env = {} if rails_env is None else {"RAILS_ENV": rails_env}
                self._assert_serial_contract(env)

    def _assert_serial_contract(self, env):
        serial = self._run("1", env_extra=env, unset=("SECRET_KEY_BASE", "RAILS_ENV"))
        serial_note = self._note("alpha")
        parallel = self._run(
            "2", env_extra={**env, "OVERLAP_SUITE_TESTS": "1"},
            unset=("SECRET_KEY_BASE", "RAILS_ENV"),
        )
        self.assertEqual(serial.returncode, 0, serial.stderr + serial.stdout)
        self.assertEqual(parallel.returncode, 0, parallel.stderr + parallel.stdout)
        self.assertEqual(serial.tally_path.read_text(), parallel.tally_path.read_text())
        self.assertEqual(serial.fail_path.read_text(), parallel.fail_path.read_text())
        noisy = parallel.tally_path.read_text().split("noisy_test|", 1)[1]
        # The first-error column is the first non-blank line of the
        # combined stream. STDERR must still precede the raise.
        self.assertIn("STDERR-BEFORE", noisy)
        self.assertNotIn("STDOUT-AFTER", noisy.split("STDERR-BEFORE", 1)[0])
        self.assertIn("PASS|test/models/alpha_test|1|1|", parallel.tally_path.read_text())
        self.assertIn("PASS|test/models/beta_test|1|1|", parallel.tally_path.read_text())
        self.assertIn("ruby files ran in", parallel.stdout)
        self.assertNotIn("ruby files ran in", serial.stdout)
        # Isolation is the overlapping write of the same storage name.
        # Both files passed, so neither saw the other's bytes.
        note = self._note("alpha")
        self.assertEqual(serial_note, note)
        self.assertIn(f"uid={os.getuid()}", note)
        self.assertIn(f"euid={os.geteuid()}", note)
        self.assertIn("secret=campfire-suite-secret", note)
        # Capability lines from the test process, not from the mount helper.
        # Inherited and ambient must be clear; the bounding set must still
        # be the caller's, which an ordinary process also has.
        ordinary = subprocess.run(
            ["ruby", "-e", 'puts File.read("/proc/self/status").lines.grep(/^Cap/)'],
            check=True,
            text=True,
            capture_output=True,
        ).stdout
        for line in note.splitlines():
            if line.startswith("Cap"):
                self.assertIn(line, ordinary.splitlines(), line)

    def test_refusing_unshare_falls_back_to_serial(self):
        self._two_file_emit()
        # Installed unshare that cannot mount. The suite must run the
        # files in this process and still pass, not skip and not fail.
        fake = Path(self.tmp.name) / "bin" / "unshare"
        executable(
            fake,
            "#!/bin/sh\n"
            "echo 'mount: permission denied' >&2\n"
            "exit 1\n",
        )
        result = self._run(
            "2",
            env_extra={"PATH": f"{fake.parent}:{os.environ['PATH']}"},
            unset=("SECRET_KEY_BASE",),
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertNotIn("ruby files ran in", result.stdout)
        self.assertIn("PASS|test/models/alpha_test|1|1|", result.tally_path.read_text())
        self.assertIn("secret=campfire-suite-secret", self._note("alpha"))
        self.assertIn(f"uid={os.getuid()}", self._note("alpha"))

    def test_missing_unshare_falls_back_to_serial(self):
        self._two_file_emit()
        missing = Path(self.tmp.name) / "missing-unshare.sh"
        # Simulate a failed command lookup without changing the suite's API
        # or hiding the other tools that a serial run needs.
        write(missing, """command() {
    if [[ "$1" == -v && "$2" == unshare ]]; then return 1; fi
    builtin command "$@"
}
""")
        result = self._run(
            "2",
            env_extra={"BASH_ENV": str(missing)},
            unset=("SECRET_KEY_BASE",),
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertNotIn("ruby files ran in", result.stdout)
        self.assertIn("PASS|test/models/alpha_test|1|1|", result.tally_path.read_text())
        self.assertIn("secret=campfire-suite-secret", self._note("alpha"))

    def test_worker_mount_failure_cannot_run_with_shared_storage(self):
        self._require_mounts()
        self._two_file_emit()
        fake = Path(self.tmp.name) / "bin" / "mount"
        executable(fake, f"""#!/bin/sh
if [ "$3" = "$REFUSED_STORAGE_ROOT" ]; then
    echo 'storage mount refused' >&2
    exit 1
fi
exec "{shutil.which('mount')}" "$@"
""")
        for root in ("tmp/storage", "storage/files"):
            with self.subTest(root=root):
                result = self._run(
                    "2", env_extra={
                        "PATH": f"{fake.parent}:{os.environ['PATH']}",
                        "REFUSED_STORAGE_ROOT": root,
                    }, unset=("SECRET_KEY_BASE",),
                )
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertIn("ruby files ran in", result.stdout)
                self.assertNotIn("PASS|", result.tally_path.read_text())
                self.assertEqual(result.tally_path.read_text().count("storage mount refused"), 3)
                self.assertFalse((self.emit / "tmp/storage/shared-name").exists())
                self.assertFalse((self.emit / "storage/files/sh/ar/shared-name").exists())


class CampfireCompareTargetTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(CAMPFIRE_COMPARE), *args],
            check=False,
            text=True,
            capture_output=True,
        )

    def test_help_documents_rust_target_and_alias(self):
        result = self._run("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--target TARGET  ruby (default), spinel, or rust", result.stdout)
        self.assertIn("--rust", result.stdout)

    def test_rust_target_is_accepted_but_gc_options_stay_spinel_only(self):
        result = self._run("--target", "rust", "--minor-gc")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--minor-gc is only valid with --target spinel", result.stdout)
        self.assertNotIn("--target must be ruby, spinel, or rust", result.stdout)

    def test_unknown_target_is_rejected_before_external_prerequisites(self):
        result = self._run("--target", "not-a-target")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--target must be ruby, spinel, or rust", result.stdout)
        self.assertNotIn("no oracle", result.stdout)


class CampfireBrowserTargetTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(CAMPFIRE_E2E), *args],
            check=False,
            text=True,
            capture_output=True,
        )

    def test_help_documents_shared_browser_targets(self):
        result = self._run("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--target T   ruby (default), spinel, or rust", result.stdout)

    def test_unknown_target_is_rejected_before_toolchain_checks(self):
        result = self._run("--target", "not-a-target")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--target must be ruby, spinel, or rust", result.stdout)
        self.assertNotIn("needs the sqlite3 CLI", result.stdout)

    def test_reuse_rejects_an_unbuilt_rust_emit(self):
        with tempfile.TemporaryDirectory() as emit:
            result = self._run("--target", "rust", "--reuse", emit)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reused emit is not a built Rust app", result.stdout)


if __name__ == "__main__":
    unittest.main()
