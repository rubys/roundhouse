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

// The transpiled shape of runtime/ruby's pluralize: the function body
// plus the wrapping `export function` signature line. No parens around
// the ternary: since the js_ast printer (5f7ea9e/e512b63),
// parenthesization is precedence-driven and `return <ternary>;` needs
// none.
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

    // The legacy Integer entry point remains byte-identical. Formatted
    // count labels use a second, concrete String entry point; neither
    // accepts any/unknown. parameterize remains in the family reopen.
    assert_eq!(emit_method(&methods[0]), EXPECTED_TS);
    assert!(emitted.starts_with(EXPECTED_TS), "emitted TS mismatch: {emitted}");
    assert!(emitted.contains("export function pluralize_formatted(count: string, word: string): string"), "{emitted}");
}

#[test]
#[ignore = "requires Bun to execute the emitted TypeScript"]
fn formatted_pluralize_executes_as_typescript() {
    let ruby = std::fs::read_to_string("runtime/ruby/inflector.rb").unwrap();
    let rbs = std::fs::read_to_string("runtime/ruby/inflector.rbs").unwrap();
    let methods = parse_methods_with_rbs(&ruby, &rbs).unwrap();
    let emitted = emit_module(&methods).unwrap();
    let probe = r#"
for (const [count, expected] of [
  ["1", "1 word"], ["1.00", "1.00 word"], ["01", "01 words"],
  ["1.01", "1.01 words"], ["1,001", "1,001 words"], ["", " words"],
  ["2\n1.0\n", "2\n1.0\n word"]
]) {
  const actual = pluralize_formatted(count, "word");
  if (actual !== expected) throw new Error(JSON.stringify({count, actual, expected}));
}
"#;
    let run = std::process::Command::new("bun").arg("-e")
        .arg(format!("{emitted}\n{probe}")).output().expect("run Bun");
    assert!(run.status.success(), "{emitted}\n{}", String::from_utf8_lossy(&run.stderr));
}

/// Phase 1 second target: errors.rb has five classes (RecordNotFound,
/// ValueTooLong, RecordNotUnique, SoleRecordExceeded, RecordInvalid), one
/// with a synth attr_reader
/// (`record`) and an `initialize` that calls `super(...)`. Validates parent extends
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

    assert_eq!(classes.len(), 7, "expected 7 classes; got {}", classes.len());

    let not_found = classes
        .iter()
        .find(|c| c.name.0.as_str() == "ActiveRecord::RecordNotFound")
        .expect("ActiveRecord::RecordNotFound");
    let nf_ts = emit_library_class(not_found).expect("emit RecordNotFound");
    assert!(
        nf_ts.contains("export class RecordNotFound extends Error"),
        "RecordNotFound: {nf_ts}"
    );

    let too_long = classes
        .iter()
        .find(|c| c.name.0.as_str() == "ActiveRecord::ValueTooLong")
        .expect("ActiveRecord::ValueTooLong");
    let tl_ts = emit_library_class(too_long).expect("emit ValueTooLong");
    assert!(
        tl_ts.contains("export class ValueTooLong extends Error"),
        "ValueTooLong: {tl_ts}"
    );

    let not_unique = classes
        .iter()
        .find(|c| c.name.0.as_str() == "ActiveRecord::RecordNotUnique")
        .expect("ActiveRecord::RecordNotUnique");
    let nu_ts = emit_library_class(not_unique).expect("emit RecordNotUnique");
    assert!(
        nu_ts.contains("export class RecordNotUnique extends Error"),
        "RecordNotUnique: {nu_ts}"
    );

    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "ActiveRecord::RecordInvalid")
        .expect("ActiveRecord::RecordInvalid");
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
    ("runtime/ruby/json_builder.rb", "runtime/ruby/json_builder.rbs"),
    ("runtime/ruby/active_record/errors.rb", "runtime/ruby/active_record/errors.rbs"),
    ("runtime/ruby/active_record/base.rb", "runtime/ruby/active_record/base.rbs"),
    ("runtime/ruby/action_view/view_helpers.rb", "runtime/ruby/action_view/view_helpers.rbs"),
    // route_helpers retired 2026-05-01: superseded by per-app
    // generation via lower_routes_to_library_functions →
    // app/route_helpers.rb. The hand-written version was a stand-in
    // for what the lowerer now produces.
    ("runtime/ruby/action_controller/base.rb", "runtime/ruby/action_controller/base.rbs"),
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

/// Ruby methods whose same-named JS native means something else.
/// `Array#sort` on numbers needs a numeric comparator (JS's default
/// compares as strings, so `[10, 9, 1].sort()` is `[1, 10, 9]`).
/// `String#gsub` with a String pattern replaces every occurrence
/// (`replaceAll`); an empty literal pattern uses `replace(/(?:)/gu, …)`
/// so steps are by code point (not UTF-16 units).
#[test]
fn native_semantics_follow_ruby() {
    let methods = parse_methods_with_rbs(
        "module Natives\n  def sorted(xs)\n    xs.sort\n  end\n  def words(xs)\n    xs.sort\n  end\n  def swap(s)\n    s.gsub(\"l\", \"L\")\n  end\n  def swap_rx(s)\n    s.gsub(/l+/, \"L\")\n  end\n  def spread(s)\n    s.gsub(\"\", \"-\")\n  end\nend\n",
        "module Natives\n  def sorted: (Array[Integer]) -> Array[Integer]\n  def words: (Array[String]) -> Array[String]\n  def swap: (String) -> String\n  def swap_rx: (String) -> String\n  def spread: (String) -> String\nend\n",
    )
    .expect("parse");
    let sorted = emit_method(&methods[0]);
    let words = emit_method(&methods[1]);
    let swap = emit_method(&methods[2]);
    let swap_rx = emit_method(&methods[3]);
    let spread = emit_method(&methods[4]);
    assert!(sorted.contains("[...xs].sort((a, b) => a - b)"), "numeric sort:\n{sorted}");
    assert!(words.contains("[...xs].sort()"), "string sort keeps the default:\n{words}");
    assert!(swap.contains("s.replaceAll(\"l\", \"L\")"), "string gsub:\n{swap}");
    assert!(swap_rx.contains("s.replace(/l+/g, \"L\")"), "regex gsub:\n{swap_rx}");
    assert!(spread.contains("s.replace(/(?:)/gu, \"-\")"), "empty gsub:\n{spread}");
}
