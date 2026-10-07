#!/usr/bin/env python3
"""Record and summarize byte-level evidence for CI download archives."""

import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path

SCHEMA = 1
SHA = re.compile(r"[0-9a-f]{40}\Z")
GROUPS = {"browse": "build-site", "campfire": "campfire-archive-build"}
CHECKS = {
    **{
        f"smoke-{target}": f"browse/{target}.tgz"
        for target in (
            "rust",
            "crystal",
            "kotlin",
            "swift",
            "csharp",
            "typescript",
            "go",
            "elixir",
            "python",
            "ruby",
            "jruby",
        )
    },
    "spinel-smoke": "browse/spinel.tgz",
    "campfire-smoke": "campfire/spinel.tgz",
    "campfire-smoke-docker": "campfire/docker.tgz",
}
CHECK_JOBS = {
    check: (
        "smoke" if path.startswith("browse/") and check != "spinel-smoke" else check
    )
    for check, path in CHECKS.items()
}


def identity():
    source = os.environ.get("GITHUB_SHA", "")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    if not SHA.fullmatch(source):
        raise ValueError("GITHUB_SHA must be a lowercase full 40-character SHA")
    if not run_id.isdigit() or int(run_id) < 1:
        raise ValueError("GITHUB_RUN_ID must be a positive integer")
    if not attempt.isdigit() or int(attempt) < 1:
        raise ValueError("GITHUB_RUN_ATTEMPT must be a positive integer")
    return source, run_id, int(attempt)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def revision(path):
    if path is None or not Path(path).is_file():
        return None
    value = Path(path).read_text(encoding="utf-8").strip()
    if not SHA.fullmatch(value):
        raise ValueError(f"invalid Spinel revision in {path}")
    return value


def write_json(path, value):
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    temporary.replace(destination)


def base(kind):
    source, run_id, attempt = identity()
    return {
        "schema": SCHEMA,
        "kind": kind,
        "source_sha": source,
        "run_id": run_id,
        "attempt": attempt,
    }


def inventory(args):
    root = Path(args.root)
    compiler = revision(args.spinel_revision_file)
    archives = []
    group_root = root / args.group
    if group_root.is_dir():
        for path in sorted(group_root.iterdir()):
            if not path.is_file() or path.suffix not in {".tgz", ".zip", ".json"}:
                continue
            logical = path.relative_to(root).as_posix()
            archives.append(
                {
                    "path": logical,
                    "sha256": digest(path),
                    "size": path.stat().st_size,
                    "spinel_revision": (
                        compiler if logical == "campfire/docker.tgz" else None
                    ),
                }
            )
    document = base("inventory")
    document.update(
        {"group": args.group, "producer_outcome": args.outcome, "archives": archives}
    )
    write_json(args.out, document)


def validation(args):
    if CHECKS[args.check] != args.path:
        raise ValueError(f"{args.check} does not validate {args.path}")
    archive = Path(args.archive)
    document = base("validation")
    document.update(
        {
            "check": args.check,
            "path": args.path,
            "sha256": digest(archive) if archive.is_file() else None,
            "size": archive.stat().st_size if archive.is_file() else None,
            "outcome": args.outcome,
            "spinel_revision": revision(args.spinel_revision_file),
        }
    )
    write_json(args.out, document)


def load_evidence(directory, source, run_id, current_attempt):
    inventories, validations = {}, {}
    for path in sorted(Path(directory).rglob("*.json")):
        try:
            item = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            raise ValueError(f"malformed evidence {path}: {error}") from error
        if not isinstance(item, dict) or item.get("schema") != SCHEMA:
            raise ValueError(f"malformed evidence {path}: unsupported schema")
        if item.get("source_sha") != source or item.get("run_id") != run_id:
            raise ValueError(f"foreign evidence {path}")
        attempt = item.get("attempt")
        if type(attempt) is not int or not 1 <= attempt <= current_attempt:
            raise ValueError(f"malformed evidence {path}: invalid attempt")
        kind = item.get("kind")
        if kind == "inventory":
            validate_inventory(item, path)
            key = item["group"]
            target = inventories
        elif kind == "validation":
            validate_validation(item, path)
            key = item["check"]
            target = validations
        else:
            raise ValueError(f"malformed evidence {path}: invalid kind")
        previous = target.get(key)
        if previous is None or item["attempt"] > previous["attempt"]:
            target[key] = item
        elif item["attempt"] == previous["attempt"] and item != previous:
            raise ValueError(f"conflicting evidence for {key} attempt {attempt}")
    return inventories, validations


def valid_hash(value):
    return value is None or bool(re.fullmatch(r"[0-9a-f]{64}", value))


def validate_inventory(item, path):
    if (
        item.get("group") not in GROUPS
        or not isinstance(item.get("producer_outcome"), str)
        or not isinstance(item.get("archives"), list)
    ):
        raise ValueError(f"malformed inventory {path}")
    seen = set()
    for archive in item["archives"]:
        if not isinstance(archive, dict):
            raise TypeError(f"malformed inventory archive {path}")
        logical = archive.get("path")
        prefix = item["group"] + "/"
        if (
            not isinstance(logical, str)
            or not logical.startswith(prefix)
            or "/" in logical[len(prefix) :]
            or Path(logical).suffix not in {".tgz", ".zip", ".json"}
            or logical in seen
            or not valid_hash(archive.get("sha256"))
            or archive.get("sha256") is None
            or type(archive.get("size")) is not int
            or archive["size"] < 0
            or (
                archive.get("spinel_revision") is not None
                and not SHA.fullmatch(archive["spinel_revision"])
            )
            or (
                logical != "campfire/docker.tgz"
                and archive.get("spinel_revision") is not None
            )
        ):
            raise ValueError(f"malformed inventory archive {path}")
        seen.add(logical)


def validate_validation(item, path):
    check = item.get("check")
    if (
        check not in CHECKS
        or item.get("path") != CHECKS.get(check)
        or not isinstance(item.get("outcome"), str)
        or not valid_hash(item.get("sha256"))
        or (item.get("size") is not None and type(item.get("size")) is not int)
        or (item.get("sha256") is None) != (item.get("size") is None)
        or (
            item.get("spinel_revision") is not None
            and not SHA.fullmatch(item["spinel_revision"])
        )
    ):
        raise ValueError(f"malformed validation {path}")


def expected_paths(plan):
    result = set()
    for value in plan.get("archives", []):
        if not isinstance(value, str):
            raise TypeError("CI_PLAN archives must be strings")
        result.add(value if "/" in value else f"browse/{value}.tgz")
    jobs = plan.get("jobs", [])
    smoke = plan.get("smoke", [])
    if not isinstance(jobs, list) or not isinstance(smoke, list):
        raise TypeError("CI_PLAN jobs and smoke must be lists")
    if "campfire-archive-build" in jobs:
        result.update(("campfire/spinel.tgz", "campfire/docker.tgz"))
    return result


def need_result(needs, job):
    value = needs.get(job)
    return value.get("result") if isinstance(value, dict) else None


def archive_status(path, producer, witness, selected, needs, attempt):
    if not selected:
        return "not-selected", None
    if producer is None:
        return "unverified", "missing producer inventory"
    if producer["producer_outcome"] != "success":
        return "failed" if producer[
            "producer_outcome"
        ] == "failure" else "unverified", (
            f"producer outcome {producer['producer_outcome']}"
        )
    producer_job = GROUPS[path.split("/", 1)[0]]
    producer_need = need_result(needs, producer_job)
    if needs and producer_need != "success":
        return "unverified", f"producer job result {producer_need or 'missing'}"
    archive = next((a for a in producer["archives"] if a["path"] == path), None)
    if archive is None:
        return "unverified", "selected archive missing"
    check = next((name for name, target in CHECKS.items() if target == path), None)
    if check is None:
        return "unverified", "no archive-specific smoke check"
    if witness is None:
        return "unverified", "selected witness missing"
    if witness["attempt"] != attempt:
        return (
            "unverified",
            "earlier attempt witness; current execution not established",
        )
    if witness["sha256"] != archive["sha256"] or witness["size"] != archive["size"]:
        return "failed", "producer and witness bytes differ"
    if (
        path in {"browse/spinel.tgz", "campfire/spinel.tgz"}
        and not witness["spinel_revision"]
    ):
        return "unverified", "native Spinel revision unknown"
    if path == "campfire/docker.tgz" and not archive["spinel_revision"]:
        return "unverified", "packaging Spinel revision unknown"
    outcome = witness["outcome"]
    check_need = need_result(needs, CHECK_JOBS[check])
    if outcome == "failure":
        return "failed", "harness outcome failure"
    # The ordinary smoke job is a matrix. One failed sibling must not erase
    # another target's actual byte-specific successful harness result.
    if needs and (check_need is None or check_need in {"skipped", "cancelled"}):
        return "unverified", f"consumer job result {check_need or 'missing'}"
    if outcome == "success":
        return "passed", None
    if outcome == "reused":
        return "reused", None
    return "unverified", f"harness outcome {outcome or 'unknown'}"


def report(args):
    source, run_id, attempt = identity()
    try:
        plan = json.loads(os.environ["CI_PLAN"])
        needs = json.loads(os.environ.get("CI_NEEDS", "{}"))
    except (KeyError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid CI plan/needs: {error}") from error
    if not isinstance(plan, dict) or not isinstance(needs, dict):
        raise TypeError("CI_PLAN and CI_NEEDS must be JSON objects")
    inventories, validations = load_evidence(args.evidence_dir, source, run_id, attempt)
    paths = expected_paths(plan)
    selected = {f"browse/{target}.tgz" for target in plan.get("smoke", [])}
    selected.update(CHECKS[job] for job in plan.get("jobs", []) if job in CHECKS)
    for producer in inventories.values():
        paths.update(archive["path"] for archive in producer["archives"])
    archives = []
    for path in sorted(paths):
        group = path.split("/", 1)[0]
        producer = inventories.get(group)
        check = next((name for name, target in CHECKS.items() if target == path), None)
        witness = validations.get(check) if check else None
        status, reason = archive_status(
            path, producer, witness, path in selected, needs, attempt
        )
        archive = (
            next((a for a in producer["archives"] if a["path"] == path), None)
            if producer
            else None
        )
        archives.append(
            {
                "path": path,
                "selected": path in selected,
                "status": status,
                "reason": reason,
                "sha256": archive["sha256"] if archive else None,
                "size": archive["size"] if archive else None,
                "check": check,
                "validation_attempt": (witness or {}).get("attempt"),
                "harness_outcome": (witness or {}).get("outcome"),
                "spinel_revision": (
                    (witness or {}).get("spinel_revision")
                    if path in {"browse/spinel.tgz", "campfire/spinel.tgz"}
                    else (archive or {}).get("spinel_revision")
                ),
            }
        )
    document = {
        "schema": SCHEMA,
        "source_sha": source,
        "run_id": run_id,
        "attempt": attempt,
        "producers": [
            {
                "group": group,
                "attempt": item["attempt"],
                "outcome": item["producer_outcome"],
                "job_result": need_result(needs, GROUPS[group]),
            }
            for group, item in sorted(inventories.items())
        ],
        "archives": archives,
    }
    write_json(args.out, document)


def verify(args):
    source, run_id, attempt = identity()
    document = json.loads(Path(args.report).read_text())
    if (
        document.get("schema") != SCHEMA
        or document.get("source_sha") != source
        or document.get("run_id") != run_id
        or document.get("attempt") != attempt
    ):
        raise ValueError("publication report has foreign identity")
    archives = {item["path"]: item for item in document["archives"]}
    root = Path(args.root)
    published = set()
    for group in GROUPS:
        directory = root / group
        if not directory.is_dir():
            continue
        for path in directory.iterdir():
            if not path.is_file() or path.suffix not in {".tgz", ".zip", ".json"}:
                continue
            logical = path.relative_to(root).as_posix()
            item = archives.get(logical)
            if (
                item is None
                or item.get("sha256") != digest(path)
                or item.get("size") != path.stat().st_size
            ):
                raise ValueError(f"untracked or changed publication bytes: {logical}")
            published.add(logical)
    # Verification checks identity, not success: failed repro bytes remain useful.
    for item in document["archives"]:
        item["published"] = item["path"] in published
    write_json(args.report, document)


def parser():
    result = argparse.ArgumentParser()
    commands = result.add_subparsers(dest="command", required=True)
    inv = commands.add_parser("inventory")
    inv.add_argument("--group", choices=sorted(GROUPS), required=True)
    inv.add_argument("--root", required=True)
    inv.add_argument("--out", required=True)
    inv.add_argument("--outcome", required=True)
    inv.add_argument("--spinel-revision-file")
    inv.set_defaults(function=inventory)
    val = commands.add_parser("validation")
    val.add_argument("--archive", required=True)
    val.add_argument("--path", choices=sorted(CHECKS.values()), required=True)
    val.add_argument("--check", choices=sorted(CHECKS), required=True)
    val.add_argument("--outcome", required=True)
    val.add_argument("--out", required=True)
    val.add_argument("--spinel-revision-file")
    val.set_defaults(function=validation)
    rep = commands.add_parser("report")
    rep.add_argument("--evidence-dir", required=True)
    rep.add_argument("--out", required=True)
    rep.set_defaults(function=report)
    pub = commands.add_parser("verify")
    pub.add_argument("--root", required=True)
    pub.add_argument("--report", required=True)
    pub.set_defaults(function=verify)
    return result


def main():
    try:
        args = parser().parse_args()
        args.function(args)
    except (OSError, TypeError, ValueError) as error:
        print(f"ci-archive-evidence: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
