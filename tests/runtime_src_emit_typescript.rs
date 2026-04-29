//! TypeScript analogue of tests/runtime_src_emit_python.rs:
//! the runtime-extraction pipeline, given the Ruby+RBS source for
//! `pluralize`, must emit TS code that matches the hand-written
//! pluralize in runtime/typescript/view_helpers.ts.

use roundhouse::emit::typescript::{emit_method, emit_module};
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
fn pluralize_emits_expected_typescript() {
    let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("parse");
    assert_eq!(methods.len(), 1);
    let emitted = emit_method(&methods[0]);
    assert_eq!(emitted, EXPECTED_TS, "emitted TS mismatch");
}

/// Phase 1 entry point: transpile the whole `runtime/ruby/inflector.rb`
/// + `inflector.rbs` pair into a TypeScript module, end-to-end.
/// Smallest file in the runtime corpus (5 LOC, one method) — validates
/// the module-level pipeline before scaling up.
#[test]
fn inflector_module_transpiles_to_typescript() {
    let ruby = std::fs::read_to_string("runtime/ruby/inflector.rb")
        .expect("read inflector.rb");
    let rbs = std::fs::read_to_string("runtime/ruby/inflector.rbs")
        .expect("read inflector.rbs");

    let methods = parse_methods_with_rbs(&ruby, &rbs).expect("parse");
    let emitted = emit_module(&methods).expect("emit_module");

    assert_eq!(emitted, EXPECTED_TS, "emitted TS mismatch");
}

/// Phase 1 gap survey: the full set of `runtime/ruby/*` files this
/// pipeline must eventually cover. Each entry is `(rb_path, rbs_path)`
/// — every framework Ruby file roundhouse has today.
const RUNTIME_PAIRS: &[(&str, &str)] = &[
    ("runtime/ruby/inflector.rb", "runtime/ruby/inflector.rbs"),
    ("runtime/ruby/active_record/errors.rb", "runtime/ruby/active_record/errors.rbs"),
    ("runtime/ruby/active_record/validations.rb", "runtime/ruby/active_record/validations.rbs"),
    ("runtime/ruby/active_record/base.rb", "runtime/ruby/active_record/base.rbs"),
    ("runtime/ruby/action_view/view_helpers.rb", "runtime/ruby/action_view/view_helpers.rbs"),
    ("runtime/ruby/action_view/route_helpers.rb", "runtime/ruby/action_view/route_helpers.rbs"),
    ("runtime/ruby/action_controller/base.rb", "runtime/ruby/action_controller/base.rbs"),
    ("runtime/ruby/action_controller/parameters.rb", "runtime/ruby/action_controller/parameters.rbs"),
    ("runtime/ruby/action_dispatch/router.rb", "runtime/ruby/action_dispatch/router.rbs"),
];

/// Empirical gap-survey for Phase 1: try the full runtime/ruby/ corpus
/// through `parse_methods_with_rbs` and `emit_module`, recording where
/// each fails. The test passes; its emitted output (visible via
/// `cargo test -- --nocapture`) is the punch list of pipeline gaps that
/// Phase 1 work should close one by one.
#[test]
fn runtime_corpus_phase1_gap_survey() {
    let mut parse_ok = 0usize;
    let mut emit_ok = 0usize;
    let mut report = String::new();

    for (rb_path, rbs_path) in RUNTIME_PAIRS {
        let ruby = match std::fs::read_to_string(rb_path) {
            Ok(s) => s,
            Err(e) => {
                report.push_str(&format!("  {rb_path}: read failed: {e}\n"));
                continue;
            }
        };
        let rbs = match std::fs::read_to_string(rbs_path) {
            Ok(s) => s,
            Err(e) => {
                report.push_str(&format!("  {rbs_path}: read failed: {e}\n"));
                continue;
            }
        };

        let parse_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parse_methods_with_rbs(&ruby, &rbs)
        }));
        let methods = match parse_result {
            Ok(Ok(m)) => {
                parse_ok += 1;
                m
            }
            Ok(Err(e)) => {
                report.push_str(&format!("  {rb_path}: PARSE error: {e}\n"));
                continue;
            }
            Err(_) => {
                report.push_str(&format!("  {rb_path}: PARSE panicked\n"));
                continue;
            }
        };

        let emit_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_module(&methods)
        }));
        match emit_result {
            Ok(Ok(_)) => {
                emit_ok += 1;
                report.push_str(&format!("  {rb_path}: OK ({} methods)\n", methods.len()));
            }
            Ok(Err(e)) => {
                report.push_str(&format!("  {rb_path}: EMIT error: {e}\n"));
            }
            Err(_) => {
                report.push_str(&format!("  {rb_path}: EMIT panicked\n"));
            }
        }
    }

    eprintln!(
        "Phase 1 gap survey: parse {}/{}, emit {}/{}",
        parse_ok, RUNTIME_PAIRS.len(), emit_ok, RUNTIME_PAIRS.len(),
    );
    eprintln!("{report}");
}
