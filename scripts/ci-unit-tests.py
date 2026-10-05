#!/usr/bin/env python3
"""Run unit CI as bounded cargo batches and free finished integration artifacts.

Preserves every --all-targets identity: library unit tests, package binaries,
and each integration target discovered from Cargo metadata. Cargo still builds
and executes each batch; results are never cached or skipped. After a batch of
integration targets finishes successfully, only that batch's integration
executables and their own split-DWARF sidecars are deleted. Shared libraries,
package binaries (including CARGO_BIN_EXE helpers), fingerprints, and dependency
artifacts stay until the job ends.

Peak disk is reduced because only one integration batch is resident at a time.
Compile-everything-before-any-execute is intentionally not preserved: a later
batch can fail to compile after earlier batches have already run.

Optional --shard-index/--shard-count stride sorted integration targets across
jobs. Coverage is checked on the global metadata set first. Shard 0 also runs
library and binary unit tests; every integration target still executes exactly
once across the shards.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

DEFAULT_BATCH = 20
PACKAGE = "roundhouse"
# Cargo names integration executables `{target}-{16 hex hash}` under deps/.
EXEC_HASH = re.compile(r"\A([0-9a-f]+)\Z")


def cargo_bin() -> str:
    return os.environ.get("CARGO", "cargo")


def run_cargo(args: list[str], *, capture: bool = False) -> int:
    command = [cargo_bin(), *args]
    if capture:
        result = subprocess.run(command, check=False, text=True, capture_output=True)
        if result.returncode != 0 and result.stderr:
            sys.stderr.write(result.stderr)
        return result.returncode if result.returncode >= 0 else 128 - result.returncode
    # Inherit stdio so cargo diagnostics and test output stream normally.
    status = subprocess.call(command)
    return status if status >= 0 else 128 - status


def cargo_metadata() -> dict:
    for offline in (True, False):
        args = ["metadata", "--format-version", "1", "--no-deps"]
        if offline:
            args.append("--offline")
        result = subprocess.run(
            [cargo_bin(), *args],
            check=False,
            text=True,
            capture_output=True,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        if not offline:
            sys.stderr.write(result.stderr or result.stdout or "cargo metadata failed\n")
            raise SystemExit(result.returncode or 1)
    raise SystemExit("cargo metadata failed")


def package_plan(meta: dict) -> tuple[list[str], Path]:
    packages = [p for p in meta["packages"] if p["name"] == PACKAGE]
    if len(packages) != 1:
        raise SystemExit(f"expected one {PACKAGE} package, found {len(packages)}")
    names = sorted(t["name"] for t in packages[0]["targets"] if "test" in t["kind"])
    if not names:
        raise SystemExit("no integration test targets in cargo metadata")
    return names, Path(meta["target_directory"])


def chunks(items: list[str], size: int) -> list[list[str]]:
    if size < 1:
        raise SystemExit("--batch-size must be >= 1")
    return [items[i : i + size] for i in range(0, len(items), size)]


def validate_shard(index: int, count: int) -> None:
    if count < 1:
        raise SystemExit("--shard-count must be >= 1")
    if not 0 <= index < count:
        raise SystemExit("--shard-index must satisfy 0 <= index < --shard-count")


def select_shard(names: list[str], index: int, count: int) -> list[str]:
    return names[index::count]


def verify_coverage(names: list[str]) -> None:
    roots = sorted(p.stem for p in Path("tests").glob("*.rs"))
    missing = [stem for stem in roots if stem not in names]
    if missing:
        raise SystemExit(
            "integration metadata missing tests/*.rs stems: " + ", ".join(missing[:20])
        )
    extra = [name for name in names if name not in roots]
    if extra:
        raise SystemExit(
            "cargo metadata lists integration targets without tests/*.rs: "
            + ", ".join(extra[:20])
        )


def integration_executables(deps: Path, target_name: str) -> list[Path]:
    """Locate Cargo integration executables for one target by deps stem."""
    if not deps.is_dir():
        return []
    prefix = target_name + "-"
    found: list[Path] = []
    for path in deps.iterdir():
        if not path.is_file() or path.suffix:
            continue
        if not path.name.startswith(prefix):
            continue
        suffix = path.name[len(prefix) :]
        if not EXEC_HASH.fullmatch(suffix):
            continue
        if not os.access(path, os.X_OK):
            continue
        found.append(path)
    return found


def owned_integration_paths(executable: Path) -> list[Path]:
    """Files that belong only to one finished integration executable.

    Unpacked .dwo sidecars sit beside the executable as `{stem}.*`. Match by
    the full executable stem so neighboring targets and package bins stay.
    """
    if not executable.is_file():
        return []
    stem = executable.name
    owned = [executable]
    for path in executable.parent.iterdir():
        name = path.name
        if name == stem:
            continue
        if name.startswith(stem + "."):
            owned.append(path)
    return owned


def free_integration_targets(deps: Path, names: list[str]) -> int:
    freed = 0
    for name in names:
        for executable in integration_executables(deps, name):
            for path in owned_integration_paths(executable):
                try:
                    size = path.stat().st_size if path.exists() else 0
                    path.unlink(missing_ok=True)
                    freed += size
                except OSError as error:
                    print(f"warning: could not free {path}: {error}", file=sys.stderr)
    return freed


def selectors(names: list[str]) -> list[str]:
    out: list[str] = []
    for name in names:
        out.extend(["--test", name])
    return out


def build_and_run_lib_bins(*, timings: bool) -> int:
    build = ["test", "--locked", "--lib", "--bins", "--no-run"]
    if timings:
        build.append("--timings")
    print("== unit batch: build library + binaries ==", flush=True)
    status = run_cargo(build)
    if status != 0:
        return status
    print("== unit batch: run library + binaries ==", flush=True)
    return run_cargo(["test", "--locked", "--lib", "--bins"])


def build_and_run_integration_batch(
    names: list[str],
    *,
    batch_index: int,
    batch_count: int,
    deps: Path,
    timings: bool,
) -> int:
    label = f"{batch_index}/{batch_count}"
    flags = selectors(names)
    print(
        f"== unit batch {label}: build {len(names)} integration target(s) ==",
        flush=True,
    )
    build = ["test", "--locked", *flags, "--no-run"]
    if timings and batch_index == 1:
        # One timings report for the first integration wave; later waves would
        # overwrite cargo-timing.html and add little signal for disk work.
        build.append("--timings")
    status = run_cargo(build)
    if status != 0:
        return status
    missing = [name for name in names if not integration_executables(deps, name)]
    if missing:
        print(
            "error: no integration executable under deps/ for: " + ", ".join(missing),
            file=sys.stderr,
        )
        return 1
    print(
        f"== unit batch {label}: run {len(names)} integration target(s) ==",
        flush=True,
    )
    status = run_cargo(["test", "--locked", *flags])
    if status != 0:
        # Keep failing artifacts for local inspection; do not free on failure.
        return status
    freed = free_integration_targets(deps, names)
    print(
        f"== unit batch {label}: freed {freed} bytes of finished integration artifacts ==",
        flush=True,
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--batch-size",
        type=int,
        default=int(os.environ.get("ROUNDHOUSE_UNIT_BATCH_SIZE", DEFAULT_BATCH)),
        help=f"integration targets per build/run/free wave (default {DEFAULT_BATCH})",
    )
    parser.add_argument(
        "--timings",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="pass --timings on the shared lib/bin build and first integration wave",
    )
    parser.add_argument(
        "--list-only",
        action="store_true",
        help="print this shard's integration targets after coverage validation and exit",
    )
    parser.add_argument(
        "--shard-index",
        type=int,
        default=0,
        help="zero-based shard to run (default 0)",
    )
    parser.add_argument(
        "--shard-count",
        type=int,
        default=1,
        help="number of shards to partition integration targets across (default 1)",
    )
    args = parser.parse_args(argv)
    validate_shard(args.shard_index, args.shard_count)

    started = time.monotonic()
    names, target_root = package_plan(cargo_metadata())
    verify_coverage(names)
    selected = select_shard(names, args.shard_index, args.shard_count)
    batches = chunks(selected, args.batch_size)
    shard_note = (
        f"shard {args.shard_index} of {args.shard_count}: "
        f"{len(selected)} selected of {len(names)} integration targets"
    )
    if args.list_only:
        for name in selected:
            print(name)
        print(
            f"# {shard_note}; {len(batches)} batch(es) of up to {args.batch_size}",
            file=sys.stderr,
        )
        return 0

    deps = target_root / "debug" / "deps"
    lib_bins = args.shard_index == 0
    print(
        f"unit CI: {shard_note}, "
        f"{len(batches)} batch(es) of up to {args.batch_size}; "
        f"{'lib+bins first; ' if lib_bins else 'lib+bins skipped; '}"
        f"reclaim finished integration artifacts only",
        flush=True,
    )

    if lib_bins:
        status = build_and_run_lib_bins(timings=args.timings)
        if status != 0:
            return status

    for index, batch in enumerate(batches, start=1):
        status = build_and_run_integration_batch(
            batch,
            batch_index=index,
            batch_count=len(batches),
            deps=deps,
            timings=args.timings,
        )
        if status != 0:
            return status

    elapsed = time.monotonic() - started
    print(f"unit CI complete in {elapsed:.1f}s", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
