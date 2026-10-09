"""Stable Cargo rustc diagnostic indexing, independent of a Rust toolchain."""

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts/campfire-rust-diagnostics.py"
spec = importlib.util.spec_from_file_location("campfire_rust_diagnostics", SCRIPT)
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)


def event(package_id="path+file:///tmp/work/Cargo.toml#campfire@0.1.0", level="error",
          code="E0308", line=12, message="mismatched types", filename="src/main.rs",
          target_name="app", kind=None):
    diagnostic = {
        "level": level,
        "message": message,
        "code": None if code is None else {"code": code, "explanation": None},
        "spans": [{
            "file_name": filename,
            "line_start": line,
            "line_end": line,
            "column_start": 3,
            "column_end": 9,
            "is_primary": True,
            "text": [{"text": "let value: i32 = source;", "highlight_start": 3, "highlight_end": 9}],
            "label": "expected `i32`",
            "expansion": None,
        }],
        "children": [],
    }
    return {
        "reason": "compiler-message",
        "package_id": package_id,
        "target": {
            "name": target_name,
            "kind": kind or ["bin"],
            "crate_types": ["bin"],
            "test": False,
            "edition": "2024",
        },
        "message": diagnostic,
    }


def parse(*events, root="/tmp/work"):
    payload = [json.dumps(item) if not isinstance(item, str) else item for item in events]
    return tool.parse_capture(payload, Path(root))


class CampfireRustDiagnosticsTests(unittest.TestCase):
    def test_dependency_and_generated_crate_remain_separate(self):
        dependency = event(package_id="registry+https://example.invalid#serde@1.0.0", filename="/tmp/deps/serde/src/lib.rs")
        generated = event()
        records, malformed = parse(dependency, generated)
        self.assertFalse(malformed)
        self.assertEqual(records[0]["package_name"], "serde")
        self.assertEqual(records[0]["target"]["name"], "app")
        self.assertEqual(records[0]["target"]["kind"], ["bin"])
        self.assertFalse(records[0]["target"]["test"])
        self.assertEqual(records[1]["package_name"], "campfire")
        self.assertNotEqual(records[0]["fingerprint"], records[1]["fingerprint"])
        self.assertEqual(records[1]["spans"][0]["file_name"], "src/main.rs")

    def test_repeated_diagnostic_groups_with_count(self):
        records, malformed = parse(event(), event())
        result = tool.inventory(records, malformed)
        self.assertEqual(result["error_count"], 2)
        self.assertEqual(result["group_count"], 1)
        self.assertEqual(result["groups"][0]["count"], 2)

    def test_line_movement_does_not_change_fingerprint_and_raw_span_is_kept(self):
        records, _ = parse(event(line=12), event(line=91))
        self.assertEqual(records[0]["fingerprint"], records[1]["fingerprint"])
        self.assertEqual(records[1]["spans"][0]["line_start"], 91)

    def test_cargo_target_metadata_is_retained_in_fingerprint(self):
        regular_target = event()
        test_harness_capable_target = event()
        test_harness_capable_target["target"]["test"] = True
        records, _ = parse(regular_target, test_harness_capable_target)
        self.assertNotEqual(records[0]["fingerprint"], records[1]["fingerprint"])
        self.assertFalse(records[0]["target"]["test"])
        self.assertTrue(records[1]["target"]["test"])

    def test_code_less_errors_are_indexed(self):
        records, malformed = parse(event(code=None, message="cannot find value `x` in this scope"))
        self.assertFalse(malformed)
        self.assertEqual(len(records), 1)
        self.assertIsNone(records[0]["code"])
        self.assertEqual(records[0]["message"], "cannot find value `x` in this scope")

    def test_warnings_are_not_counted(self):
        records, malformed = parse(event(level="warning"))
        self.assertEqual(records, [])
        self.assertEqual(tool.inventory(records, malformed)["error_count"], 0)

    def test_malformed_json_reports_line_number(self):
        records, malformed = parse(event(), "{bad json", "[]", event(line=20))
        self.assertEqual(len(records), 2)
        self.assertEqual(malformed[0]["line"], 2)
        self.assertTrue(malformed[0]["error"])
        self.assertEqual(malformed[1]["line"], 3)
        self.assertEqual(malformed[1]["error"], "expected a Cargo JSON object")
        self.assertEqual(tool.inventory(records, malformed)["malformed"][0]["line"], 2)


if __name__ == "__main__":
    unittest.main()
