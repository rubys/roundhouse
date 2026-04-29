//! TypeScript analogue of tests/runtime_src_emit_python.rs:
//! the runtime-extraction pipeline, given the Ruby+RBS source for
//! `pluralize`, must emit TS code that matches the hand-written
//! pluralize in runtime/typescript/view_helpers.ts.

use roundhouse::emit::typescript::emit_method;
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

// Matches runtime/typescript/view_helpers.ts:383-385 (the function
// body) plus the wrapping `export function` signature line.
const EXPECTED_TS: &str = "\
export function pluralize(count: number, word: string): string {
  return count === 1 ? `1 ${word}` : `${count} ${word}s`;
}
";

#[test]
#[ignore = "TS rip-and-replace migration"]
fn pluralize_emits_expected_typescript() {
    let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("parse");
    assert_eq!(methods.len(), 1);
    let emitted = emit_method(&methods[0]);
    assert_eq!(emitted, EXPECTED_TS, "emitted TS mismatch");
}
