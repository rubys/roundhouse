#!/usr/bin/env python3
"""Build one CI test archive, then run a checked case partition without rebuilding."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMAT_VERSION = 1
Case = tuple[str, str]


class ContractError(ValueError):
    pass


class ChildFailure(Exception):
    def __init__(self, status: int):
        self.status = status if status >= 0 else 128 - status


def check_command(command: list[str], *, env: dict | None = None, capture: bool = False) -> str:
    result = subprocess.run(command, env=env, cwd=ROOT, text=True,
                            stdout=subprocess.PIPE if capture else None, check=False)
    if result.returncode:
        raise ChildFailure(result.returncode)
    return result.stdout if capture else ""


def nextest(*args: str) -> list[str]:
    return [os.environ.get("CARGO", "cargo"), "nextest", *args]


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def validate_shard(index: int, count: int) -> None:
    if type(count) is not int or count < 1:
        raise ContractError("shard count must be a positive integer")
    if type(index) is not int or not 0 <= index < count:
        raise ContractError("shard index must satisfy 0 <= index < shard count")


def parse_inventory(data: dict, *, partitioned: bool = False) -> tuple[dict[Case, bool], set[Case]]:
    """Retain ignored identities; only explicitly matched active cases may run."""
    if not isinstance(data, dict) or not isinstance(data.get("rust-suites"), dict):
        raise ContractError("missing Nextest rust-suites inventory")
    cases: dict[Case, bool] = {}
    selected: set[Case] = set()
    bundled = False
    for binary, suite in data["rust-suites"].items():
        if (not isinstance(binary, str) or not binary or not isinstance(suite, dict)
                or suite.get("binary-id") != binary or suite.get("status") != "listed"
                or not isinstance(suite.get("testcases"), dict)):
            raise ContractError("malformed or unlisted Nextest suite")
        if suite.get("binary-name") == "all_tests" and suite.get("kind") == "test":
            bundled = bool(suite["testcases"])
        for name, case in suite["testcases"].items():
            if (not isinstance(name, str) or not name or not isinstance(case, dict)
                    or case.get("kind") != "test" or type(case.get("ignored")) is not bool
                    or not isinstance(case.get("filter-match"), dict)):
                raise ContractError("malformed Nextest test identity")
            identity = (binary, name)
            if identity in cases:
                raise ContractError("duplicate Nextest test identity")
            ignored = case["ignored"]
            match = case["filter-match"]
            status = match.get("status")
            if status not in {"matches", "mismatch"}:
                raise ContractError("unknown Nextest filter status")
            if ignored:
                if status == "matches":
                    raise ContractError("an ignored test was selected")
            elif status == "matches":
                selected.add(identity)
            elif not partitioned or match.get("reason") != "partition":
                raise ContractError(f"active test omitted by a non-partition filter: {identity}")
            cases[identity] = ignored
    if not bundled:
        raise ContractError("combined all_tests suite is missing or empty")
    if type(data.get("test-count")) is not int or data["test-count"] != len(cases):
        raise ContractError("Nextest test-count does not match its identities")
    if not any(not ignored for ignored in cases.values()):
        raise ContractError("test inventory has no active cases")
    return cases, selected


def encode_cases(cases: dict[Case, bool]) -> list[dict]:
    return [{"binary": binary, "name": name, "ignored": cases[(binary, name)]}
            for binary, name in sorted(cases)]


def manifest_cases(manifest: dict) -> dict[Case, bool]:
    required = {"version", "source_sha", "workspace", "target", "archive_sha256", "shard_count", "tests"}
    if (not isinstance(manifest, dict) or set(manifest) != required
            or type(manifest["version"]) is not int or manifest["version"] != FORMAT_VERSION):
        raise ContractError("unsupported unit inventory manifest")
    for key, lengths in (("source_sha", (40, 64)), ("archive_sha256", (64,))):
        value = manifest[key]
        if not isinstance(value, str) or len(value) not in lengths or re.fullmatch(r"[0-9a-f]+", value) is None:
            raise ContractError(f"invalid {key} in unit inventory")
    for key in ("workspace", "target"):
        if not isinstance(manifest[key], str) or not Path(manifest[key]).is_absolute():
            raise ContractError(f"invalid {key} in unit inventory")
    validate_shard(0, manifest["shard_count"])
    if not isinstance(manifest["tests"], list) or not manifest["tests"]:
        raise ContractError("empty unit inventory manifest")
    cases: dict[Case, bool] = {}
    for row in manifest["tests"]:
        if (not isinstance(row, dict) or set(row) != {"binary", "name", "ignored"}
                or not isinstance(row["binary"], str) or not row["binary"]
                or not isinstance(row["name"], str) or not row["name"]
                or type(row["ignored"]) is not bool):
            raise ContractError("malformed unit inventory identity")
        identity = (row["binary"], row["name"])
        if identity in cases:
            raise ContractError("duplicate unit inventory identity")
        cases[identity] = row["ignored"]
    if not any(not ignored for ignored in cases.values()):
        raise ContractError("unit inventory has no active cases")
    return cases


def validate_manifest(manifest: dict, *, workspace: Path, target: Path, source: str,
                      archive_sha256: str, shard_count: int) -> dict[Case, bool]:
    cases = manifest_cases(manifest)
    if manifest["shard_count"] != shard_count:
        raise ContractError("shard count differs from producer inventory")
    if (Path(manifest["workspace"]).resolve() != workspace.resolve()
            or Path(manifest["target"]).resolve() != target.resolve()):
        raise ContractError("archive paths differ from this checkout; compile-time paths cannot be remapped")
    if manifest["source_sha"] != source:
        raise ContractError("archive source SHA differs from this checkout")
    if manifest["archive_sha256"] != archive_sha256:
        raise ContractError("archive digest differs from producer inventory")
    return cases


def check_partition_coverage(cases: dict[Case, bool], partitions: list[set[Case]]) -> None:
    active = {identity for identity, ignored in cases.items() if not ignored}
    covered: set[Case] = set()
    for partition in partitions:
        if covered & partition:
            raise ContractError("test partitions overlap")
        if not partition <= active:
            raise ContractError("test partition selects unknown or ignored cases")
        covered.update(partition)
    if covered != active:
        raise ContractError("test partitions omit active cases")


def source_sha() -> str:
    locations = {"GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE",
                 "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"}
    env = {key: value for key, value in os.environ.items() if key not in locations}
    return check_command(["git", "rev-parse", "HEAD"], env=env, capture=True).strip()


def target_directory() -> Path:
    target = ROOT / "target"
    configured = Path(os.environ.get("CARGO_TARGET_DIR", str(target)))
    if not configured.is_absolute():
        configured = ROOT / configured
    if configured.resolve() != target.resolve():
        raise ContractError("CI archives require CARGO_TARGET_DIR to be workspace/target")
    return target.resolve()


def list_tests(args: list[str], *, env: dict | None = None) -> dict:
    return json.loads(check_command(nextest("list", *args, "--profile", "ci", "--ignore-default-filter",
                                             "--run-ignored", "default", "--message-format=json"),
                                    env=env, capture=True))


def build(args: argparse.Namespace) -> int:
    validate_shard(0, args.shard_count)
    target = target_directory()
    env = dict(os.environ, ROUNDHOUSE_TEST_BUNDLE_ROOT=str(ROOT), CARGO_TARGET_DIR=str(target))
    if env.get("CARGO_PROFILE_TEST_SPLIT_DEBUGINFO", "off") != "off":
        raise ContractError("CI archives require test split-debuginfo=off for embedded backtrace line data")
    env.setdefault("CARGO_PROFILE_TEST_SPLIT_DEBUGINFO", "off")
    archive = args.archive.resolve()
    inventory = args.inventory.resolve()
    archive.parent.mkdir(parents=True, exist_ok=True)
    # Reduce the serial project build while keeping dependencies optimized.
    selectors = [
        "--locked", "--lib", "--bins", "--test", "all_tests",
        "--config", "profile.test.package.roundhouse.opt-level=0",
    ]
    command = nextest("archive", *selectors, "--profile", "ci", "--archive-file", str(archive))
    if args.timings:
        command.append("--timings")
    check_command(command, env=env)
    data = list_tests(selectors, env=env)
    cases, _selected = parse_inventory(data)
    if Path(data["rust-build-meta"]["target-directory"]).resolve() != target:
        raise ContractError("built test target directory differs from the archive contract")
    write_json(inventory, {
        "version": FORMAT_VERSION, "source_sha": source_sha(), "workspace": str(ROOT),
        "target": str(target), "archive_sha256": digest(archive), "shard_count": args.shard_count,
        "tests": encode_cases(cases),
    })
    print(f"unit archive: {len(cases)} identities, {sum(not ignored for ignored in cases.values())} active")
    return 0


def run(args: argparse.Namespace, report: dict) -> int:
    validate_shard(args.shard_index, args.shard_count)
    target = target_directory()
    manifest = json.loads(args.inventory.read_text())
    expected = validate_manifest(manifest, workspace=ROOT, target=target, source=source_sha(),
                                 archive_sha256=digest(args.archive), shard_count=args.shard_count)
    extracted = list_tests(["--archive-file", str(args.archive.resolve()), "--extract-to", str(ROOT),
                            "--extract-overwrite"])
    actual, _selected = parse_inventory(extracted)
    if actual != expected:
        raise ContractError("extracted tests or ignored states differ from producer inventory")
    metadata = ["--cargo-metadata", str(target / "nextest/cargo-metadata.json"),
                "--binaries-metadata", str(target / "nextest/binaries-metadata.json")]
    partitions = []
    for index in range(args.shard_count):
        listed = list_tests([*metadata, "--partition", f"hash:{index + 1}/{args.shard_count}"])
        identities, selected = parse_inventory(listed, partitioned=True)
        if identities != expected:
            raise ContractError("partition inventory differs from producer inventory")
        partitions.append(selected)
    check_partition_coverage(expected, partitions)
    selected = partitions[args.shard_index]
    if not selected:
        raise ContractError("assigned test partition is empty")
    report.update(coverage_verified=True, source_sha=manifest["source_sha"], total_cases=len(expected),
                  ignored_cases=sum(expected.values()), selected_cases=len(selected),
                  selected=[{"binary": binary, "name": name} for binary, name in sorted(selected)])
    command = nextest("run", *metadata, "--profile", "ci", "--partition",
                      f"hash:{args.shard_index + 1}/{args.shard_count}", "--no-fail-fast",
                      "--ignore-default-filter", "--run-ignored", "default", "--retries", "0", "--no-tests", "fail")
    check_command(command)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    producer = modes.add_parser("build", help="build one archive and its complete test inventory")
    producer.add_argument("--archive", type=Path, required=True)
    producer.add_argument("--inventory", type=Path, required=True)
    producer.add_argument("--shard-count", type=int, default=4)
    producer.add_argument("--timings", action=argparse.BooleanOptionalAction, default=True)
    consumer = modes.add_parser("run", help="validate and run one case partition from an archive")
    consumer.add_argument("--archive", type=Path, required=True)
    consumer.add_argument("--inventory", type=Path, required=True)
    consumer.add_argument("--shard-index", type=int, required=True)
    consumer.add_argument("--shard-count", type=int, required=True)
    consumer.add_argument("--report", type=Path, required=True)
    args = parser.parse_args(argv)
    report = {"status": "not-run", "coverage_verified": False}
    if args.mode == "run":
        report.update(shard_index=args.shard_index, shard_count=args.shard_count)
    try:
        status = build(args) if args.mode == "build" else run(args, report)
    except ChildFailure as error:
        status = error.status
    except (ContractError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"unit CI: {error}", file=sys.stderr)
        report["error"] = str(error)
        status = 2
    report.update(status="passed" if status == 0 else "failed", exit=status)
    if args.mode == "run":
        write_json(args.report, report)
    return status


if __name__ == "__main__":
    raise SystemExit(main())
