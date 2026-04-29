//! TypeScript analogue of tests/runtime_src_emit_python.rs:
//! the runtime-extraction pipeline, given the Ruby+RBS source for
//! `pluralize`, must emit TS code that matches the hand-written
//! pluralize in runtime/typescript/view_helpers.ts.

use roundhouse::emit::typescript::{emit_library_class, emit_method, emit_module};
use roundhouse::runtime_src::{parse_library_with_rbs, parse_methods_with_rbs};

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

/// Phase 1 second target: errors.rb has two classes (RecordNotFound,
/// RecordInvalid), one with a synth attr_reader (`record`) and an
/// `initialize` that calls `super(...)`. Validates parent extends
/// (StandardError → Error), constructor synthesis, attr_reader-as-
/// field detection, and `@ivar` → `this.x` in the constructor body.
#[test]
fn errors_rb_transpiles_to_typescript_classes() {
    let ruby = std::fs::read("runtime/ruby/active_record/errors.rb")
        .expect("read errors.rb");
    let rbs = std::fs::read_to_string("runtime/ruby/active_record/errors.rbs")
        .expect("read errors.rbs");

    let classes = parse_library_with_rbs(&ruby, &rbs, "runtime/ruby/active_record/errors.rb")
        .expect("parse_library_with_rbs");

    assert_eq!(classes.len(), 2, "expected 2 classes; got {}", classes.len());

    let not_found = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordNotFound")
        .expect("RecordNotFound");
    let nf_ts = emit_library_class(not_found).expect("emit RecordNotFound");
    assert!(
        nf_ts.contains("export class RecordNotFound extends Error"),
        "RecordNotFound: {nf_ts}"
    );

    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordInvalid")
        .expect("RecordInvalid");
    let inv_ts = emit_library_class(invalid).expect("emit RecordInvalid");
    assert!(
        inv_ts.contains("export class RecordInvalid extends Error"),
        "missing class header: {inv_ts}"
    );
    assert!(
        inv_ts.contains("record: Base;"),
        "missing field declaration from synth attr_reader: {inv_ts}"
    );
    assert!(
        !inv_ts.contains("record(): Base"),
        "synth attr_reader getter should be dropped, not emitted as method: {inv_ts}"
    );
    assert!(
        inv_ts.contains("constructor(record: Base)"),
        "missing constructor: {inv_ts}"
    );
    assert!(
        inv_ts.contains("this.record = record"),
        "missing ivar assignment in constructor: {inv_ts}"
    );
    assert!(
        inv_ts.contains("super("),
        "missing super call: {inv_ts}"
    );
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
/// through both pipelines (`parse_methods_with_rbs` + `emit_module`
/// for the flat path; `parse_library_with_rbs` + `emit_library_class`
/// for the class-shape path). A file is "covered" if either pipeline
/// emits without error. The test passes; its output (visible via
/// `cargo test -- --nocapture`) is the punch list.
#[test]
fn runtime_corpus_phase1_gap_survey() {
    let mut parse_ok = 0usize;
    let mut emit_ok = 0usize;
    let mut report = String::new();

    for (rb_path, rbs_path) in RUNTIME_PAIRS {
        let ruby_bytes = match std::fs::read(rb_path) {
            Ok(b) => b,
            Err(e) => {
                report.push_str(&format!("  {rb_path}: read failed: {e}\n"));
                continue;
            }
        };
        let ruby = match std::str::from_utf8(&ruby_bytes) {
            Ok(s) => s.to_string(),
            Err(e) => {
                report.push_str(&format!("  {rb_path}: not UTF-8: {e}\n"));
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

        // Module-flat path.
        let module_outcome: Result<usize, String> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| parse_methods_with_rbs(&ruby, &rbs)),
        ) {
            Ok(Ok(methods)) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || emit_module(&methods),
            )) {
                Ok(Ok(_)) => Ok(methods.len()),
                Ok(Err(e)) => Err(format!("module-emit: {e}")),
                Err(_) => Err("module-emit panicked".to_string()),
            },
            Ok(Err(e)) => Err(format!("module-parse: {e}")),
            Err(_) => Err("module-parse panicked".to_string()),
        };

        // Library-class path.
        let library_outcome: Result<usize, String> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| parse_library_with_rbs(&ruby_bytes, &rbs, rb_path)),
        ) {
            Ok(Ok(classes)) => {
                let mut emit_err: Option<String> = None;
                for c in &classes {
                    if let Err(e) = emit_library_class(c) {
                        emit_err = Some(format!("class `{}`: {e}", c.name.0.as_str()));
                        break;
                    }
                }
                if let Some(e) = emit_err {
                    Err(format!("library-emit: {e}"))
                } else {
                    Ok(classes.len())
                }
            }
            Ok(Err(e)) => Err(format!("library-parse: {e}")),
            Err(_) => Err("library-parse panicked".to_string()),
        };

        match (&module_outcome, &library_outcome) {
            (Ok(n), _) => {
                parse_ok += 1;
                emit_ok += 1;
                report.push_str(&format!("  {rb_path}: OK via module ({n} methods)\n"));
            }
            (_, Ok(n)) => {
                parse_ok += 1;
                emit_ok += 1;
                report.push_str(&format!("  {rb_path}: OK via library ({n} classes)\n"));
            }
            (Err(me), Err(le)) => {
                report.push_str(&format!("  {rb_path}: BOTH failed\n"));
                report.push_str(&format!("    module: {me}\n"));
                report.push_str(&format!("    library: {le}\n"));
            }
        }
    }

    eprintln!(
        "Phase 1 gap survey: parse {}/{}, emit {}/{}",
        parse_ok, RUNTIME_PAIRS.len(), emit_ok, RUNTIME_PAIRS.len(),
    );
    eprintln!("{report}");
}
