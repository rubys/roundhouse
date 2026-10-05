#!/usr/bin/env python3
"""PR-local execution receipts; uncertainty always executes.

This is not a dependency cache or a general CI scheduler. The allowlist below
owns the command contract; tests/ci_reuse_test.py exercises its trust boundaries.
"""

import argparse
import hashlib
import io
import json
import os
import stat
import subprocess
import time
import tomllib
import zipfile
from pathlib import Path

SCHEMA = 1
JOBS = {
    "store-check": {
        "checks": [
            "cargo build",
            "The analyzer reports no errors, no warnings and no ingest gaps on the store",
        ],
        "tests": [],
        "reports": [],
    },
    "writebook-inventory": {
        "checks": ["Build Roundhouse and run inventory", "Save complete check report"],
        "tests": ["tests/writebook.rs"],
        "reports": ["writebook-check.txt", "writebook-inventory-current.json"],
    },
    "smoke-rust": {
        "name": "smoke (rust)",
        "checks": ["scripts/smoke rust"],
        "reports": [],
        "consumer": ["scripts/smoke", "e2e/"],
    },
    "browser-smoke-typescript": {
        "checks": ["Run SharedWorker browser smoke (emit → vite build → drive)"],
        "reports": [],
        "consumer": ["tests/browser_smoke/"],
    },
    "rust-inflector": {
        "name": "compare (rust)",
        "checks": ["cargo test --test framework_tests_rust (green subset)"],
        "reports": [],
        "consumer": ["tests/framework_tests_rust.rs"],
    },
}
MAX_RECEIPT = 64 * 1024
MAX_BUNDLE = 16 * 1024 * 1024


def command(*args, timeout=60):
    return subprocess.check_output(args, timeout=timeout)


def digest(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def repository_inputs(job):
    # Unknown paths, all shared test support, fixtures, docs and executable
    # READMEs are included. Only unrelated top-level Rust test binaries are
    # excluded: these two commands neither compile nor execute them.
    entries = []
    for entry in command("git", "ls-tree", "-rz", "HEAD").split(b"\0"):
        if not entry:
            continue
        metadata, path = entry.split(b"\t", 1)
        path = path.decode()
        if "consumer" in JOBS[job]:
            # Producers still execute. Their consumed output is hashed below;
            # producer source edits need not invalidate identical outputs.
            inputs = [
                "scripts/ci-reuse.py",
                "scripts/ci-playwright-install",
                "scripts/ci-apt-bound",
                "scripts/ci-apt-install",
                ".github/workflows/",
                ".github/actions/",
                ".cargo/",
                "tests/support/",
                *JOBS[job]["consumer"],
            ]
            if not any(
                path == item or path.startswith(item) and item.endswith("/")
                for item in inputs
            ):
                continue
            entries.append([path, metadata.decode()])
            continue
        if (
            path.startswith("tests/")
            and path.count("/") == 1
            and path.endswith(".rs")
            and path not in JOBS[job]["tests"]
        ):
            continue
        entries.append([path, metadata.decode()])
    return digest(entries)


def file_digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def tree_digest(root, *, action_code=False, exclude=(), tool_links=False):
    # Hash actual consumed trees, not tar metadata or generator recipes. No
    # normalization of generated credentials, migration names or source text.
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        raise ValueError("missing input directory")
    entries = []
    for path in sorted(root.rglob("*")):
        mode = path.lstat().st_mode
        name = path.relative_to(root).as_posix()
        if any(name == item or name.startswith(item + "/") for item in exclude):
            continue
        if stat.S_ISLNK(mode):
            # npm's .bin links are tools, not source. Permit only contained
            # file links, recording both the link and the target it executes.
            if not tool_links:
                raise ValueError("symlink in external input")
            try:
                target = path.resolve(strict=True)
            except RuntimeError as error:
                raise ValueError("cyclic tool link") from error
            if not target.is_relative_to(root.resolve()) or not target.is_file():
                raise ValueError("escaping or directory tool link")
            entries.append(
                [
                    name,
                    "link",
                    os.readlink(path),
                    stat.S_IMODE(target.stat().st_mode),
                    file_digest(target),
                ]
            )
            continue
        if stat.S_ISREG(mode):
            # The runner writes _actions/<owner>/<repo>/<ref>.completed with
            # the current download time. Only that adjacent, known marker is
            # metadata; *.completed inside action source remains an input.
            if (
                action_code
                and len(path.relative_to(root).parts) == 3
                and path.name.endswith(".completed")
                and path.with_suffix("").is_dir()
            ):
                continue
            entries.append(
                [
                    name,
                    stat.S_IMODE(mode),
                    file_digest(path),
                ]
            )
        elif stat.S_ISDIR(mode):
            entries.append([name, stat.S_IMODE(mode)])
        else:
            raise ValueError("unsupported external file type")
    if not entries:
        raise ValueError("empty input directory")
    return digest(entries)


def environment_inputs():
    # Base toolchain witness. Browser consumers add their installed packages
    # and payloads below. Observe actual tools/image, not moving tag names.
    image = {
        key: os.environ[key]
        for key in ("ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH")
    }
    actions = Path(os.environ["RUNNER_WORKSPACE"]).parent / "_actions"
    # An allowlist, not CARGO_*: registry credentials must never enter receipts.
    flag_names = (
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTUP_TOOLCHAIN",
        "RUST_MIN_STACK",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CC",
        "CXX",
        "AR",
        "LD",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "LIBCLANG_PATH",
        "CLANG_PATH",
        "LD_LIBRARY_PATH",
    )
    flags = {key: os.environ[key] for key in flag_names if key in os.environ}
    # rust-cache sets CARGO_INCREMENTAL=0. No run-specific GitHub token/context
    # is persisted.
    return {
        "image": image,
        "actions": tree_digest(actions, action_code=True),
        "rustc": command("rustc", "-vV").decode(),
        "cargo": command("cargo", "-V").decode(),
        "cc": command("cc", "--version").decode(),
        "flags": flags,
    }


def consumer_inputs(job, input_path, resolved=None):
    """Complete witnessed consumer inputs, also recomputed after execution."""
    root = Path(resolved or input_path)
    environment = environment_inputs()
    # These workflows fix their consumer configuration. Unexpected selection
    # or interpreter overrides disable reuse rather than persisting secrets.
    for flag in (
        "E2E_SKIP",
        "SKIP_EMIT",
        "NODE_OPTIONS",
        "NODE_PATH",
        "SMOKE_MIN_TESTS",
        "SELENIUM_REMOTE_URL",
    ):
        if os.environ.get(flag):
            raise ValueError("unexpected consumer override")
    environment["ci"] = os.environ["CI"]
    if job == "smoke-rust" and any(
        key in os.environ for key in ("DATABASE_PATH", "PORT", "DATABASE_POOL_SIZE")
    ):
        raise ValueError("unwitnessed server/database override")
    environment["consumer_flags"] = {
        key: os.environ[key]
        for key in ("NODE_ENV", "TZ", "LANG", "LC_ALL")
        if key in os.environ
    }
    # Cargo registry overrides/path sources are not covered by registry locks.
    for flag in os.environ:
        if flag.startswith(("CARGO_REGISTRIES_", "CARGO_SOURCE_")):
            raise ValueError("external Cargo source configuration")
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    if any((cargo_home / name).exists() for name in ("config", "config.toml")):
        raise ValueError("unwitnessed global Cargo configuration")
    if job == "browser-smoke-typescript":
        exclude = ("node_modules",)
        modules = [root / "node_modules", Path("tests/browser_smoke/node_modules")]
    else:
        manifest = tomllib.loads((root / "Cargo.toml").read_text())
        if (
            set(manifest)
            - {"package", "lib", "bin", "dependencies", "dev-dependencies"}
            or "build" in manifest.get("package", {})
            or (root / ".cargo").exists()
            or manifest.get("lib", {}).get("path", "src/lib.rs") != "src/lib.rs"
            or any(
                binary.get("path", "src/main.rs") != "src/main.rs"
                for binary in manifest.get("bin", [])
            )
        ):
            raise ValueError("unaudited Cargo manifest/configuration contract")
        for group in ("dependencies", "dev-dependencies"):
            for dependency in manifest.get(group, {}).values():
                if isinstance(dependency, dict) and any(
                    key in dependency
                    for key in ("git", "path", "registry", "workspace")
                ):
                    raise ValueError("unwitnessed Cargo dependency")
        if (root / "build.rs").exists():
            raise ValueError("unwitnessed Cargo build contract")
        lock = tomllib.loads((root / "Cargo.lock").read_text())
        for package in lock["package"]:
            if "source" not in package and any(
                package.get(key) != manifest["package"].get(key)
                for key in ("name", "version")
            ):
                raise ValueError("unwitnessed local Cargo package")
            if "source" in package and (
                package["source"]
                != "registry+https://github.com/rust-lang/crates.io-index"
                or "checksum" not in package
            ):
                raise ValueError("unwitnessed Cargo package source")
        exclude = ("target",)
        modules = []
        if job == "smoke-rust":
            exclude += (
                "e2e/node_modules",
                "e2e/test-results",
                "e2e/playwright-report",
                "storage/development.sqlite3",
                "storage/development.sqlite3-wal",
                "storage/development.sqlite3-shm",
            )
            modules = [root / "e2e/node_modules"]
    if modules:
        environment.update(
            node=command("node", "-p", "JSON.stringify(process.versions)").decode(),
            npm=command("npm", "--version").decode(),
            system_packages=hashlib.sha256(command("dpkg-query", "-W")).hexdigest(),
            modules=[tree_digest(path, tool_links=True) for path in modules],
        )
        # A fixed, explicit root is shared with installation AND Playwright.
        # .links tracks package paths for install-time browser garbage
        # collection; it is not loaded when a prepared browser executes.
        browsers = Path(os.environ["PLAYWRIGHT_BROWSERS_PATH"])
        if not browsers.is_absolute():
            raise ValueError("browser root must be explicit and absolute")
        environment["browsers"] = tree_digest(
            browsers, exclude=(".links",), tool_links=True
        )
        environment["sqlite3"] = command("sqlite3", "--version").decode()
    result = {
        "repository": repository_inputs(job),
        "source": tree_digest(root, exclude=exclude),
        "environment": environment,
    }
    if job == "smoke-rust":
        result["archive"] = file_digest(input_path)
    return result


def execution_inputs(job, input_path, resolved=None):
    if "consumer" in JOBS[job]:
        return consumer_inputs(job, input_path, resolved)
    return {
        "repository": repository_inputs(job),
        "source": tree_digest(input_path),
        "environment": environment_inputs(),
    }


def prepare_archive(input_path, destination):
    # A SEPARATE extraction resolves the inputs. Never copy its locks/modules
    # into the pristine extraction that must prove the README actually works.
    destination.mkdir(parents=True)
    command("tar", "xzf", str(Path(input_path).resolve()), "-C", str(destination))
    root = destination / "rust"
    import re

    blocks_dir = destination / ".smoke-blocks"
    # The same parser extracts the blocks that pristine smoke executes.
    command(
        str(Path(__file__).with_name("smoke")),
        "--extract-blocks",
        str(root / "README.md"),
        str(blocks_dir),
    )
    blocks = [
        (path.name, path.read_text().strip()) for path in sorted(blocks_dir.iterdir())
    ]
    if blocks != [
        ("01--Build.sh", "cargo build --release"),
        ("02--Setup.sh", "sqlite3 storage/development.sqlite3 < db/seed.sql"),
        ("03--Test.sh", "cargo test"),
        (
            "04--End-to-end.sh",
            "cd e2e\nnpm install\nnpx playwright install chromium\nnpx playwright test",
        ),
    ]:
        raise ValueError("unaudited archive README command contract")
    npm = json.loads((root / "e2e/package.json").read_text())
    if (
        npm.get("scripts") != {"test": "playwright test"}
        or set(npm["devDependencies"]) != {"@playwright/test"}
        or not re.fullmatch(
            r"\d+\.\d+\.\d+", npm["devDependencies"]["@playwright/test"]
        )
    ):
        raise ValueError("unaudited archive npm contract")
    command(
        "cargo",
        "generate-lockfile",
        "--manifest-path",
        str(root / "Cargo.toml"),
        timeout=240,
    )
    # Inherit the same explicit browser root as the validation job.
    subprocess.run(
        ["npm", "install", "--no-audit", "--no-fund"],
        cwd=root / "e2e",
        check=True,
        timeout=240,
    )
    subprocess.run(
        ["npx", "playwright", "install", "chromium"],
        cwd=root / "e2e",
        check=True,
        timeout=240,
    )
    return root


class GitHub:
    def __init__(self, repo):
        self.repo = repo
        self.deadline = time.monotonic() + 90

    def request(self, path):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("reuse lookup time budget exceeded")
        return command(
            "gh", "api", f"repos/{self.repo}/{path}", timeout=min(15, remaining)
        )

    def get(self, path):
        return json.loads(self.request(path))

    def pages(self, path, key):
        separator = "&" if "?" in path else "?"
        for page in range(1, 11):
            items = self.get(f"{path}{separator}per_page=100&page={page}")[key]
            yield from items
            if len(items) < 100:
                return
        raise ValueError("pagination bound exceeded")

    def bundle(self, artifact):
        if not 0 < artifact["size_in_bytes"] <= MAX_BUNDLE:
            raise ValueError("oversized receipt artifact")
        data = self.request(f"actions/artifacts/{artifact['id']}/zip")
        if len(data) > MAX_BUNDLE:
            raise ValueError("oversized receipt bundle")
        return data


def read_bundle(data, job):
    # Never extract an old artifact into the workspace or execute its contents.
    # Only the receipt and explicitly named nonexecutable reports are accepted.
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        files = archive.infolist()
        allowed = {"receipt.json", *JOBS[job]["reports"]}
        if len(files) != len(allowed) or {f.filename for f in files} != allowed:
            raise ValueError("unexpected receipt bundle paths")
        if sum(f.file_size for f in files) > MAX_BUNDLE:
            raise ValueError("oversized uncompressed bundle")
        for item in files:
            if stat.S_ISLNK(item.external_attr >> 16):
                raise ValueError("symlink in receipt bundle")
        receipt_data = archive.read("receipt.json")
        if len(receipt_data) > MAX_RECEIPT:
            raise ValueError("oversized receipt")
        receipt = json.loads(receipt_data)
        reports = {name: archive.read(name) for name in JOBS[job]["reports"]}
        expected = {
            name: hashlib.sha256(data).hexdigest() for name, data in reports.items()
        }
        if receipt.get("reports") != expected:
            raise ValueError("report digest mismatch")
        return receipt, reports


def successful_execution(jobs, receipt, job):
    name = JOBS[job].get("name", job)
    matches = [candidate for candidate in jobs if candidate["name"] == name]
    if len(matches) != 1:
        return None
    candidate = matches[0]
    if candidate["status"] != "completed" or candidate["conclusion"] != "success":
        return None
    for name in JOBS[job]["checks"]:
        steps = [step for step in candidate["steps"] if step["name"] == name]
        if len(steps) != 1 or steps[0]["conclusion"] != "success":
            return None
    # These jobs have no continue-on-error validation steps. The receipt also
    # records their raw outcomes so adding masking cannot bless a failed test.
    if receipt.get("outcomes") != ["success"] * len(JOBS[job]["checks"]):
        return None
    return candidate


def find_execution(api, current, job):
    # Bounded history lookup is an optimization, never a requirement. A miss
    # beyond these 20 runs executes. Branch/repository plus the receipt's PR
    # identity work even when GitHub omits pull_requests for a fork run.
    from urllib.parse import urlencode

    query = urlencode(
        {"event": "pull_request", "branch": current["branch"], "per_page": 20}
    )
    runs = api.get(f"actions/workflows/{current['workflow_id']}/runs?{query}")[
        "workflow_runs"
    ]
    for run in runs:
        if (
            run["id"] >= current["run_id"]
            or run["event"] != "pull_request"
            or run["workflow_id"] != current["workflow_id"]
            or run["head_branch"] != current["branch"]
            or run["head_repository"]["full_name"] != current["head_repository"]
        ):
            continue
        # An overall failed/cancelled run can still contain a successful job.
        for artifact in api.pages(f"actions/runs/{run['id']}/artifacts", "artifacts"):
            prefix = f"ci-executed-{job}-"
            if artifact["expired"] or not artifact["name"].startswith(prefix):
                continue
            try:
                receipt, reports = read_bundle(api.bundle(artifact), job)
                attempt = receipt["attempt"]
                identity = (
                    "schema",
                    "repository",
                    "pull_request",
                    "head_repository",
                    "branch",
                    "workflow_id",
                    "job",
                    "fingerprint",
                )
                if (
                    any(receipt.get(key) != current[key] for key in identity)
                    or receipt.get("run_id") != run["id"]
                    or type(attempt) is not int
                    or not 1 <= attempt <= run["run_attempt"]
                    or artifact["name"] != f"{prefix}{attempt}"
                    or receipt.get("executed") is not True
                ):
                    continue
                jobs = list(
                    api.pages(
                        f"actions/runs/{run['id']}/attempts/{attempt}/jobs", "jobs"
                    )
                )
                evidence = successful_execution(jobs, receipt, job)
                if evidence:
                    return evidence["html_url"], reports
            except (ValueError, KeyError, TypeError, zipfile.BadZipFile):
                continue
    return None


def output(key, value):
    with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
        stream.write(f"{key}={value}\n")


def summary(message):
    print(message)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as stream:
        stream.write(message + "\n")


def state_dir(job):
    return Path(os.environ["RUNNER_TEMP"]) / f"ci-executed-{job}"


def probe(job, input_dir):
    output("hit", "false")
    output("eligible", "false")
    if (
        os.environ.get("GITHUB_EVENT_NAME") != "pull_request"
        or os.environ.get("GITHUB_REF") == "refs/heads/main"
    ):
        summary("CI reuse disabled: main and non-PR runs always execute.")
        return
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    pr = event["pull_request"]
    api = GitHub(os.environ["GITHUB_REPOSITORY"])
    run_id = int(os.environ["GITHUB_RUN_ID"])
    run = api.get(f"actions/runs/{run_id}")
    root = state_dir(job)
    root.mkdir(parents=True, exist_ok=True)
    resolved = (
        prepare_archive(input_dir, root / "resolver") if job == "smoke-rust" else None
    )
    inputs = execution_inputs(job, input_dir, resolved)
    current = {
        "schema": SCHEMA,
        "repository": os.environ["GITHUB_REPOSITORY"],
        "pull_request": event["number"],
        "head_repository": pr["head"]["repo"]["full_name"],
        "branch": pr["head"]["ref"],
        "workflow_id": run["workflow_id"],
        "job": job,
        "run_id": run_id,
        "attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
        "merge_sha": command("git", "rev-parse", "HEAD").decode().strip(),
        "inputs": inputs,
        "local": {"input": str(Path(input_dir).resolve())},
    }
    current["fingerprint"] = digest(current["inputs"])
    output("eligible", "true")
    output("bundle", str(root / "bundle"))
    # A manual rerun is the escape hatch: it always executes and can produce
    # new execution evidence for that exact attempt.
    api.deadline = time.monotonic() + 90
    found = find_execution(api, current, job) if current["attempt"] == 1 else None
    current["reused"] = bool(found)
    (root / "state.json").write_text(json.dumps(current))
    if found:
        url, reports = found
        for name, data in reports.items():
            Path(name).write_bytes(data)
        # Set the hit only after every report has been restored successfully.
        output("hit", "true")
        summary(
            f"**Reused {job}** from [successful execution]({url}); identical input fingerprint `{current['fingerprint']}`. No new execution receipt is issued."
        )
    else:
        summary(
            f"**Executing {job}**: no matching successful execution (or manual rerun). Input fingerprint `{current['fingerprint']}`."
        )


def record(job, outcomes, validation_dir=None):
    if outcomes != ["success"] * len(JOBS[job]["checks"]):
        raise ValueError("validation did not execute successfully")
    root = state_dir(job)
    receipt = json.loads((root / "state.json").read_text())
    if receipt["reused"]:
        raise ValueError("reused validation cannot mint execution evidence")
    if job == "smoke-rust" and not validation_dir:
        raise ValueError("missing pristine validation witness")
    # Every contract re-witnesses source, environment AND repository inputs.
    current = execution_inputs(job, receipt["local"]["input"], validation_dir)
    if current != receipt["inputs"]:
        raise ValueError("inputs changed during validation")
    del receipt["local"]
    bundle = root / "bundle"
    bundle.mkdir()
    receipt.update(executed=True, outcomes=outcomes, reports={})
    for name in JOBS[job]["reports"]:
        data = Path(name).read_bytes()
        receipt["reports"][name] = hashlib.sha256(data).hexdigest()
        (bundle / name).write_bytes(data)
    (bundle / "receipt.json").write_text(json.dumps(receipt, sort_keys=True))
    output("recorded", "true")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("probe", "record"))
    parser.add_argument("--job", required=True, choices=JOBS)
    parser.add_argument("--input")
    parser.add_argument("--validation-dir")
    parser.add_argument("--outcome", action="append", default=[])
    args = parser.parse_args()
    try:
        if args.operation == "probe":
            probe(args.job, args.input)
        else:
            record(args.job, args.outcome, args.validation_dir)
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
        zipfile.BadZipFile,
    ) as error:
        # No lookup/receipt failure may fail the original validation. Returning
        # nonzero makes the workflow's explicit probe-outcome guard execute.
        print(
            f"::warning::CI reuse unavailable ({type(error).__name__}); execute normally."
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
