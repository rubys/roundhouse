#!/usr/bin/env python3
"""Compile spinel_toolchain (and tiny_api) once; shards restore binaries and run.

Mirrors the unit-build contract: producer and consumer must share the same
absolute workspace path because `env!("CARGO_BIN_EXE_roundhouse")` and the
test binary path are baked at compile time. Build artifacts are shared;
test results are never reused.

Contract helpers (`ContractError`, `digest`, `source_sha`, `target_directory`,
…) intentionally twin `scripts/ci-unit-tests.py` for now; extracting a shared
`ci_archive_contract` module is a follow-up once both producers are green on
this compile-once shape.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMAT_VERSION = 1
SHA = re.compile(r"^[0-9a-f]{40}$|^[0-9a-f]{64}$")


class ContractError(ValueError):
    pass


class ChildFailure(Exception):
    def __init__(self, status: int):
        self.status = status if status >= 0 else 128 - status


def check_command(command: list[str], *, env: dict | None = None, capture: bool = False) -> str:
    result = subprocess.run(
        command,
        env=env,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        check=False,
    )
    if result.returncode:
        raise ChildFailure(result.returncode)
    return result.stdout if capture else ""


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def source_sha() -> str:
    locations = {
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    }
    env = {key: value for key, value in os.environ.items() if key not in locations}
    return check_command(["git", "rev-parse", "HEAD"], env=env, capture=True).strip()


def target_directory() -> Path:
    target = ROOT / "target"
    configured = Path(os.environ.get("CARGO_TARGET_DIR", str(target)))
    if not configured.is_absolute():
        configured = ROOT / configured
    if configured.resolve() != target.resolve():
        raise ContractError("spinel toolchain archives require CARGO_TARGET_DIR to be workspace/target")
    return target.resolve()


def cargo_executable(test_name: str, *, env: dict) -> Path:
    """Return the executable path cargo produced for `--test <name>`."""
    raw = check_command(
        [
            os.environ.get("CARGO", "cargo"),
            "test",
            "--locked",
            "--test",
            test_name,
            "--no-run",
            "--message-format=json",
        ],
        env=env,
        capture=True,
    )
    executable = None
    for line in raw.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if not isinstance(message, dict):
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        if not isinstance(target, dict) or target.get("name") != test_name:
            continue
        path = message.get("executable")
        if isinstance(path, str) and path:
            executable = Path(path)
    if executable is None or not executable.is_file():
        raise ContractError(f"cargo did not produce an executable for --test {test_name}")
    try:
        relative = executable.resolve().relative_to(ROOT.resolve())
    except ValueError as error:
        raise ContractError(f"{test_name} executable is outside the workspace: {executable}") from error
    return relative


def list_ignored_tests(binary: Path) -> list[str]:
    raw = check_command(
        [str(ROOT / binary), "--ignored", "--list", "--format", "terse"],
        capture=True,
    )
    names = []
    for line in raw.splitlines():
        if line.endswith(": test"):
            name = line[: -len(": test")]
            if name:
                names.append(name)
    if not names:
        raise ContractError(f"no ignored tests listed by {binary}")
    if len(names) != len(set(names)):
        raise ContractError(f"duplicate ignored test names from {binary}")
    return names


def relative_file(path: Path, label: str) -> str:
    resolved = path.resolve()
    try:
        relative = resolved.relative_to(ROOT.resolve())
    except ValueError as error:
        raise ContractError(f"{label} is outside the workspace: {resolved}") from error
    if not resolved.is_file():
        raise ContractError(f"{label} is missing: {resolved}")
    return relative.as_posix()


def validate_inventory(data: dict) -> dict:
    required = {
        "version",
        "source_sha",
        "workspace",
        "target",
        "archive_sha256",
        "spinel_toolchain",
        "roundhouse",
        "tiny_api",
        "tests",
    }
    if not isinstance(data, dict) or set(data) != required:
        raise ContractError("unsupported spinel toolchain inventory")
    if type(data["version"]) is not int or data["version"] != FORMAT_VERSION:
        raise ContractError("unsupported spinel toolchain inventory version")
    if not isinstance(data["source_sha"], str) or not SHA.fullmatch(data["source_sha"]):
        raise ContractError("invalid source_sha in spinel toolchain inventory")
    for key in ("workspace", "target"):
        if not isinstance(data[key], str) or not Path(data[key]).is_absolute():
            raise ContractError(f"invalid {key} in spinel toolchain inventory")
    if not isinstance(data["archive_sha256"], str) or len(data["archive_sha256"]) != 64:
        raise ContractError("invalid archive_sha256 in spinel toolchain inventory")
    if re.fullmatch(r"[0-9a-f]{64}", data["archive_sha256"]) is None:
        raise ContractError("invalid archive_sha256 in spinel toolchain inventory")
    for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
        value = data[key]
        if not isinstance(value, dict) or set(value) != {"path", "sha256"}:
            raise ContractError(f"malformed {key} binary record")
        if (
            not isinstance(value["path"], str)
            or not value["path"]
            or Path(value["path"]).is_absolute()
            or ".." in Path(value["path"]).parts
        ):
            raise ContractError(f"invalid {key} path")
        if not isinstance(value["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) is None:
            raise ContractError(f"invalid {key} digest")
    if not isinstance(data["tests"], list) or not data["tests"]:
        raise ContractError("spinel toolchain inventory has no tests")
    seen = set()
    for name in data["tests"]:
        if not isinstance(name, str) or not name or name in seen:
            raise ContractError("malformed or duplicate spinel toolchain test name")
        seen.add(name)
    return data


def build(args: argparse.Namespace) -> int:
    target = target_directory()
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    if env.get("CARGO_PROFILE_TEST_SPLIT_DEBUGINFO", "off") != "off":
        raise ContractError("spinel toolchain archives require test split-debuginfo=off")
    env.setdefault("CARGO_PROFILE_TEST_SPLIT_DEBUGINFO", "off")
    env.setdefault("CARGO_PROFILE_TEST_OPT_LEVEL", "0")

    spinel_rel = cargo_executable("spinel_toolchain", env=env)
    tiny_rel = cargo_executable("tiny_api", env=env)
    roundhouse = ROOT / "target/debug/roundhouse"
    if not roundhouse.is_file():
        check_command(
            [os.environ.get("CARGO", "cargo"), "build", "--locked", "--bin", "roundhouse"],
            env=env,
        )
    roundhouse_rel = Path(relative_file(roundhouse, "roundhouse"))

    tests = list_ignored_tests(spinel_rel)
    archive = args.archive.resolve()
    inventory_path = args.inventory.resolve()
    archive.parent.mkdir(parents=True, exist_ok=True)

    staging = archive.parent / "spinel-toolchain-stage"
    if staging.exists():
        shutil.rmtree(staging)
    for relative in (spinel_rel, tiny_rel, roundhouse_rel):
        destination = staging / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, destination)
        destination.chmod(destination.stat().st_mode | 0o111)

    with tarfile.open(archive, "w:gz") as tar:
        for path in sorted(staging.rglob("*")):
            if path.is_file():
                tar.add(path, arcname=path.relative_to(staging).as_posix())

    inventory = {
        "version": FORMAT_VERSION,
        "source_sha": source_sha(),
        "workspace": str(ROOT.resolve()),
        "target": str(target),
        "archive_sha256": digest(archive),
        "spinel_toolchain": {"path": spinel_rel.as_posix(), "sha256": digest(ROOT / spinel_rel)},
        "roundhouse": {"path": roundhouse_rel.as_posix(), "sha256": digest(ROOT / roundhouse_rel)},
        "tiny_api": {"path": tiny_rel.as_posix(), "sha256": digest(ROOT / tiny_rel)},
        "tests": tests,
    }
    validate_inventory(inventory)
    write_json(inventory_path, inventory)
    shutil.rmtree(staging)
    print(
        f"spinel toolchain archive: {len(tests)} ignored tests; "
        f"binaries={spinel_rel.as_posix()}, {tiny_rel.as_posix()}, {roundhouse_rel.as_posix()}"
    )
    return 0


def restore(args: argparse.Namespace) -> int:
    target = target_directory()
    inventory = validate_inventory(json.loads(args.inventory.read_text()))
    archive = args.archive.resolve()
    if digest(archive) != inventory["archive_sha256"]:
        raise ContractError("archive digest differs from producer inventory")
    if inventory["source_sha"] != source_sha():
        raise ContractError("archive source SHA differs from this checkout")
    if Path(inventory["workspace"]).resolve() != ROOT.resolve():
        raise ContractError(
            "archive workspace differs from this checkout; compile-time paths cannot be remapped"
        )
    if Path(inventory["target"]).resolve() != target:
        raise ContractError("archive target directory differs from this checkout")

    expected = {inventory[key]["path"] for key in ("spinel_toolchain", "roundhouse", "tiny_api")}
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        names = {member.name for member in members}
        # len check rejects duplicate member names (a set alone would not).
        if (
            names != expected
            or len(members) != len(expected)
            or not all(member.isfile() for member in members)
        ):
            raise ContractError("archive members differ from producer inventory")
        for member in members:
            # Reject path traversal even if inventory validation was bypassed.
            if Path(member.name).is_absolute() or ".." in Path(member.name).parts:
                raise ContractError(f"invalid archive member path: {member.name}")
            destination = ROOT / member.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            source = tar.extractfile(member)
            if source is None:
                raise ContractError(f"unreadable archive member: {member.name}")
            with source, destination.open("wb") as sink:
                shutil.copyfileobj(source, sink)

    for key in ("spinel_toolchain", "roundhouse", "tiny_api"):
        record = inventory[key]
        path = ROOT / record["path"]
        if not path.is_file():
            raise ContractError(f"restored {key} binary missing: {path}")
        if digest(path) != record["sha256"]:
            raise ContractError(f"restored {key} digest differs from inventory")
        path.chmod(path.stat().st_mode | 0o111)

    # Re-list from the restored binary so a stale inventory cannot hide cases.
    listed = list_ignored_tests(Path(inventory["spinel_toolchain"]["path"]))
    if listed != inventory["tests"]:
        raise ContractError("restored binary's ignored tests differ from producer inventory")

    if args.env_file:
        path = Path(args.env_file)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "\n".join(
                [
                    f"SPINEL_TOOLCHAIN_BIN={ROOT / inventory['spinel_toolchain']['path']}",
                    f"TINY_API_BIN={ROOT / inventory['tiny_api']['path']}",
                    f"ROUNDHOUSE_BIN={ROOT / inventory['roundhouse']['path']}",
                    "",
                ]
            )
        )
    print(f"spinel toolchain restored: {len(listed)} ignored tests")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    producer = modes.add_parser("build", help="compile binaries and write archive + inventory")
    producer.add_argument("--archive", type=Path, required=True)
    producer.add_argument("--inventory", type=Path, required=True)
    consumer = modes.add_parser("restore", help="validate archive and restore binaries into workspace")
    consumer.add_argument("--archive", type=Path, required=True)
    consumer.add_argument("--inventory", type=Path, required=True)
    consumer.add_argument("--env-file", type=Path)
    args = parser.parse_args(argv)
    try:
        return build(args) if args.mode == "build" else restore(args)
    except ChildFailure as error:
        return error.status
    except (ContractError, OSError, ValueError, KeyError, TypeError, tarfile.TarError) as error:
        print(f"spinel toolchain CI: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
