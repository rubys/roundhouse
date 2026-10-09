#!/usr/bin/env python3
"""Index errors from a Cargo --message-format=json capture.

Fingerprints group similar compiler diagnostics; they are not root-cause
clusters. Confirm causes manually against generated source and the compiler
context. Input is JSON Lines emitted by Cargo, not rustc's rendered text.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path


def normalize_path(value: str, generated_root: Path | None) -> str:
    """Make workspace/temp prefixes stable without erasing relative paths."""
    path = Path(value)
    if not path.is_absolute():
        return value.replace("\\", "/")
    resolved = Path(value).resolve(strict=False)
    if generated_root is not None:
        try:
            return resolved.relative_to(generated_root.resolve(strict=False)).as_posix()
        except ValueError:
            pass
    parts = resolved.parts
    for marker in ("tmp", "var", "folders"):
        if marker in parts:
            index = parts.index(marker)
            if marker == "tmp" and index + 1 < len(parts):
                return "<tmp>/" + "/".join(parts[index + 2 :])
    return resolved.as_posix()


def normalize_message(message: str, generated_root: Path | None) -> str:
    # Rust diagnostics embed paths in message text (notably include_str! errors).
    candidates = sorted({match.group(0) for match in re.finditer(r"(?:/[^\s:'\"<>]+)+", message)}, key=len, reverse=True)
    for candidate in candidates:
        normalized = normalize_path(candidate.rstrip(".,)"), generated_root)
        message = message.replace(candidate.rstrip(".,)"), normalized)
    return " ".join(message.split())


def package_name(package_id: str | None) -> str | None:
    if package_id is None:
        return None
    # Cargo package IDs are commonly `name version (source)` or `path+...#name@version`.
    tail = package_id.rsplit("#", 1)[-1]
    return tail.split("@", 1)[0].split(" ", 1)[0] or package_id


def span_identity(span: dict, generated_root: Path | None) -> dict:
    identity = {}
    filename = span.get("file_name")
    if filename:
        identity["file"] = normalize_path(filename, generated_root)
    # Deliberately omit line/column offsets: moving code should not change identity.
    if span.get("text"):
        identity["source"] = " ".join(item.get("text", "") for item in span["text"])
    if span.get("label"):
        identity["label"] = span["label"]
    return identity


def diagnostic_record(message: dict, generated_root: Path | None) -> dict:
    diagnostic = message["message"]
    package_id = message.get("package_id")
    target = message.get("target") or {}
    code = diagnostic.get("code")
    code = code.get("code") if isinstance(code, dict) else code
    spans = diagnostic.get("spans") or []
    children = diagnostic.get("children") or []
    fingerprint_data = {
        "package": package_name(package_id),
        "target": target.get("name"),
        "kind": sorted(target.get("kind") or []),
        "test_target": target.get("test"),
        "code": code,
        "message": normalize_message(diagnostic.get("message", ""), generated_root),
        "spans": [span_identity(span, generated_root) for span in spans],
        "children": [
            {
                "message": normalize_message(child.get("message", ""), generated_root),
                "spans": [span_identity(span, generated_root) for span in child.get("spans", [])],
            }
            for child in children
        ],
    }
    fingerprint = hashlib.sha256(
        json.dumps(fingerprint_data, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return {
        "fingerprint": fingerprint,
        "package_id": package_id,
        "package_name": package_name(package_id),
        "target": {
            key: target[key]
            for key in ("name", "kind", "crate_types", "test", "doc", "doctest", "edition")
            if key in target
        },
        "message": diagnostic.get("message", ""),
        "code": code,
        "spans": spans,
        "children": children,
        "rendered": message.get("rendered"),
        "level": diagnostic.get("level"),
        "details": diagnostic,
    }


def parse_capture(lines, generated_root: Path | None = None) -> tuple[list[dict], list[dict]]:
    errors, malformed = [], []
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as exc:
            malformed.append({"line": line_number, "error": exc.msg})
            continue
        if not isinstance(event, dict):
            malformed.append({"line": line_number, "error": "expected a Cargo JSON object"})
            continue
        if event.get("reason") != "compiler-message":
            continue
        diagnostic = event.get("message") or {}
        if diagnostic.get("level") != "error":
            continue
        errors.append(diagnostic_record(event, generated_root))
    return errors, malformed


def inventory(errors: list[dict], malformed: list[dict]) -> dict:
    grouped = {}
    for record in errors:
        group = grouped.setdefault(record["fingerprint"], {"fingerprint": record["fingerprint"], "count": 0, "diagnostics": []})
        group["count"] += 1
        group["diagnostics"].append(record)
    groups = [grouped[key] for key in sorted(grouped)]
    return {
        "schema_version": 1,
        "description": "Compiler diagnostic index; fingerprint groups are not confirmed root causes.",
        "error_count": len(errors),
        "group_count": len(groups),
        "groups": groups,
        "malformed": malformed,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", nargs="?", default="-", help="Cargo JSON-lines file, or - for stdin")
    parser.add_argument("--generated-root", type=Path, help="generated project root; paths beneath it become relative")
    args = parser.parse_args(argv)
    try:
        if args.capture == "-":
            errors, malformed = parse_capture(sys.stdin, args.generated_root)
        else:
            with open(args.capture, encoding="utf-8") as source:
                errors, malformed = parse_capture(source, args.generated_root)
    except OSError as exc:
        parser.error(str(exc))
    print(json.dumps(inventory(errors, malformed), indent=2, sort_keys=True))
    for item in malformed:
        print(f"malformed JSON at line {item['line']}: {item['error']}", file=sys.stderr)
    return 2 if malformed else 0


if __name__ == "__main__":
    raise SystemExit(main())
