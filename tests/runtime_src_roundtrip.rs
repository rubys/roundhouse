//! Full runtime-extraction pipeline round-trip:
//! Ruby+RBS → typed MethodDef → Ruby source → re-parsed MethodDef.
//! The two MethodDefs should be equivalent (bodies identical, names
//! and param lists preserved; signature re-attached from the same RBS).

use roundhouse::dialect::MethodDef;
use roundhouse::emit::ruby::emit_method;
use roundhouse::runtime_src::parse_methods_with_rbs;

const PLURALIZE_RB: &str =
    "module Inflector\n  def pluralize(count, word)\n    count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\n  end\nend\n";
const PLURALIZE_RBS: &str =
    "module Inflector\n  def pluralize: (Integer, String) -> String\nend\n";

fn reparse(ruby_source: &str, rbs: &str) -> Vec<MethodDef> {
    parse_methods_with_rbs(ruby_source, rbs).expect("re-parses cleanly")
}

#[test]
fn pluralize_round_trips() {
    let original = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("typed parse");
    assert_eq!(original.len(), 1);

    let emitted_ruby = emit_method(&original[0]);
    // Sanity-check on the emitted shape — not a strict byte match, but it
    // must be valid Ruby that re-parses.
    assert!(emitted_ruby.contains("def pluralize(count, word)"));
    assert!(emitted_ruby.contains("if "));
    assert!(emitted_ruby.ends_with("end\n"));

    let reparsed = reparse(&emitted_ruby, PLURALIZE_RBS);
    assert_eq!(reparsed.len(), 1);

    // Names, params, receiver preserved.
    assert_eq!(reparsed[0].name, original[0].name);
    assert_eq!(reparsed[0].params, original[0].params);
    assert_eq!(reparsed[0].receiver, original[0].receiver);

    // Bodies: emit_expr ignores spans, and spans are all synthetic on
    // both sides of the round-trip, so bodies should compare equal.
    assert_eq!(reparsed[0].body, original[0].body);

    // Signature re-attaches from the same RBS.
    assert_eq!(reparsed[0].signature, original[0].signature);
}

#[test]
fn round_trip_is_stable_under_repeated_emit() {
    // If we emit → re-parse → emit again, the second emit's text should
    // match the first emit's text byte-for-byte (no drift per iteration).
    let first = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("parse");
    let emit1 = emit_method(&first[0]);

    let second = reparse(&emit1, PLURALIZE_RBS);
    let emit2 = emit_method(&second[0]);

    assert_eq!(emit1, emit2, "emitted text should be a fixed point");
}

#[test]
fn multi_method_round_trip() {
    let ruby = "module M\n  def a(x)\n    x\n  end\n  def b(y)\n    y\n  end\nend\n";
    let rbs = "module M\n  def a: (Integer) -> Integer\n  def b: (String) -> String\nend\n";

    let original = parse_methods_with_rbs(ruby, rbs).expect("parse");
    assert_eq!(original.len(), 2);

    // Emit each, concatenate into a single module-shaped file.
    let mut emitted = String::from("module M\n");
    for m in &original {
        // Simple per-line indent to nest each def inside the module.
        for line in emit_method(m).lines() {
            emitted.push_str("  ");
            emitted.push_str(line);
            emitted.push('\n');
        }
    }
    emitted.push_str("end\n");

    let reparsed = reparse(&emitted, rbs);
    assert_eq!(reparsed.len(), 2);
    assert_eq!(
        reparsed.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(reparsed[0].body, original[0].body);
    assert_eq!(reparsed[1].body, original[1].body);
}
