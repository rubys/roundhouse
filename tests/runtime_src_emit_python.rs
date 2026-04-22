//! Verify that the runtime-extraction pipeline, given the naive
//! Ruby+RBS source for `pluralize`, emits Python code that matches the
//! hand-written `pluralize` in runtime/python/view_helpers.py. This is
//! the "behavior-preserving probe" commit of the inflector plan:
//! same behavior, single source of truth.

use roundhouse::emit::python::emit_method;
use roundhouse::runtime_src::parse_methods_with_rbs;

const PLURALIZE_RB: &str = "\
module Inflector
  def pluralize(count, word)
    count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"
  end
end
";

const PLURALIZE_RBS: &str = "\
module Inflector
  def pluralize: (Integer, String) -> String
end
";

// Expected output, matching the function body as currently written at
// runtime/python/view_helpers.py:313 (line 313-314, `def pluralize`).
// The hand-written version uses `int`/`str` type hints and the Python
// ternary form; our emitter should produce the same.
const EXPECTED_PY: &str = "\
def pluralize(count: int, word: str) -> str:
    return f\"1 {word}\" if count == 1 else f\"{count} {word}s\"
";

#[test]
fn pluralize_emits_expected_python() {
    let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("parse");
    assert_eq!(methods.len(), 1);
    let emitted = emit_method(&methods[0]);
    assert_eq!(emitted, EXPECTED_PY, "emitted Python mismatch");
}

#[test]
fn pluralize_emitted_matches_hand_written_runtime() {
    // Confirms the emitter output is a drop-in for the hand-written
    // version shipped at runtime/python/view_helpers.py. We compare
    // against a canonical snippet rather than the file itself to keep
    // the test stable if the runtime file grows or reorders.
    let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("parse");
    let emitted = emit_method(&methods[0]);
    let handwritten = "\
def pluralize(count: int, word: str) -> str:
    return f\"1 {word}\" if count == 1 else f\"{count} {word}s\"
";
    assert_eq!(emitted.trim(), handwritten.trim());
}
