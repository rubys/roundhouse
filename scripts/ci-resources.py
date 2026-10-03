#!/usr/bin/env python3
"""Sample a Linux CI phase without caching results or changing its command.

CPU, available RAM and filesystem space describe the whole runner, not only
the child. Child CPU is cumulative; Linux ru_maxrss is the largest individual
process, NOT concurrent process-tree RAM. No command arguments/env are saved.
"""

import argparse
import csv
import json
import os
import resource
import subprocess
import sys
import time
from pathlib import Path


def cpu_times():
    # guest/guest_nice are already included in user/nice; do not count twice.
    return list(map(int, Path("/proc/stat").read_text().splitlines()[0].split()[1:9]))


def cpu_percent(before, after):
    delta = [end - start for start, end in zip(before, after)]
    total = sum(delta)
    if total <= 0:
        return 0.0, 0.0
    return round(100 * (total - delta[3] - delta[4]) / total, 2), round(
        100 * delta[4] / total, 2
    )


def sample(start, previous_cpu):
    current_cpu = cpu_times()
    busy, iowait = cpu_percent(previous_cpu, current_cpu)
    memory = {
        line.split(":")[0]: int(line.split()[1]) * 1024
        for line in Path("/proc/meminfo").read_text().splitlines()
        if line.startswith(("MemTotal:", "MemAvailable:"))
    }
    disk = os.statvfs(".")
    return {
        "elapsed_s": round(time.monotonic() - start, 3),
        "machine_cpu_busy_pct": busy,
        "machine_cpu_iowait_pct": iowait,
        "machine_mem_total_bytes": memory["MemTotal"],
        "machine_mem_available_bytes": memory["MemAvailable"],
        "disk_used_bytes": (disk.f_blocks - disk.f_bfree) * disk.f_frsize,
        "disk_available_bytes": disk.f_bavail * disk.f_frsize,
    }, current_cpu


DEPS_SAMPLE_INTERVAL_S = 60


def cargo_debug_root():
    # Match the unit job's default layout. Custom CARGO_TARGET_DIR is rare in CI.
    root = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    return root / "debug"


def cargo_dir_bytes(name):
    path = cargo_debug_root() / name
    if not path.is_dir():
        return None
    try:
        size = subprocess.check_output(["du", "-s", "-B1", str(path)], text=True)
    except (OSError, subprocess.CalledProcessError):
        # Reclaim can unlink entries while du walks; skip this sample.
        return None
    return int(size.split()[0])


def collect(child, out, start):
    out.parent.mkdir(parents=True, exist_ok=True)
    row, previous_cpu = sample(start, cpu_times())
    rows = [row]
    # Peak deps during the phase: batch reclaim makes end size unrepresentative.
    # Full `du` on multi-GiB deps is expensive — sample about once a minute.
    deps_samples = []
    next_deps_sample = 0.0
    if (size := cargo_dir_bytes("deps")) is not None:
        deps_samples.append(size)
        next_deps_sample = DEPS_SAMPLE_INTERVAL_S
    with out.with_suffix(".csv").open("w") as output:
        writer = csv.DictWriter(output, fieldnames=row.keys())
        writer.writeheader()
        writer.writerow(row)
        output.flush()
        while True:
            try:
                status = child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                status = None
            row, previous_cpu = sample(start, previous_cpu)
            rows.append(row)
            writer.writerow(row)
            output.flush()
            if status is not None or row["elapsed_s"] >= next_deps_sample:
                if (size := cargo_dir_bytes("deps")) is not None:
                    deps_samples.append(size)
                next_deps_sample = row["elapsed_s"] + DEPS_SAMPLE_INTERVAL_S
            if status is not None:
                break
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    report = {
        "checkout_sha": os.environ.get("GITHUB_SHA"),
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "exit_code": status if status >= 0 else 128 - status,
        "wall_s": row["elapsed_s"],
        "child_user_s": usage.ru_utime,
        "child_system_s": usage.ru_stime,
        "child_max_single_process_rss_bytes": usage.ru_maxrss * 1024,
        "machine_min_available_ram_bytes": min(r["machine_mem_available_bytes"] for r in rows),
        "disk_min_available_bytes": min(r["disk_available_bytes"] for r in rows),
        "disk_used_start_bytes": rows[0]["disk_used_bytes"],
        "disk_used_end_bytes": row["disk_used_bytes"],
        "disk_used_peak_bytes": max(r["disk_used_bytes"] for r in rows),
        "cargo_artifacts_bytes": {},
    }
    # Final allocated sizes, plus deps peak across samples. Never delete builds.
    for name in ["deps", "incremental", "build"]:
        size = cargo_dir_bytes(name)
        if size is not None:
            report["cargo_artifacts_bytes"][name] = size
    if deps_samples:
        report["cargo_artifacts_bytes"]["deps_peak"] = max(deps_samples)
    out.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Resources ({out.name}): {json.dumps(report)}", flush=True)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as output:
            output.write(f"\n### Resources: {out.name}\n\n```json\n")
            output.write(json.dumps(report, indent=2) + "\n```\n")
            output.write("CPU/RAM/disk samples describe the whole runner; RSS is the "
                         "largest individual child process, not concurrent tree RAM.\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path, help="report filename stem")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a command is required after --")
    start = time.monotonic()
    child = subprocess.Popen(command)
    try:
        collect(child, args.out, start)
    except (OSError, subprocess.SubprocessError) as error:
        # Disk exhaustion, unavailable /proc metrics or failed report writes
        # must not replace the command result or abandon a running child.
        print(f"Resource measurement unavailable: {error}", file=sys.stderr)
    status = child.wait()
    return status if status >= 0 else 128 - status


if __name__ == "__main__":
    raise SystemExit(main())
