use std::path::Path;
use std::process::Command;

use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(body: &str) -> String {
    let tree = [(
        "app/models/thing.rb",
        format!("class Thing < ApplicationRecord\n  def probe(flag)\n    {body}\n  end\nend\n"),
    )]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    let at = out.find("def probe(flag)\n").unwrap_or_else(|| panic!("no probe:\n{out}"));
    let body = &out[at + "def probe(flag)\n".len()..];
    body[..body.find("\n  end\n").unwrap()].trim().to_string()
}

#[test]
fn boolean_type_cast_grounds_to_the_runtime() {
    assert_eq!(emit("ActiveModel::Type::Boolean.new.cast(flag)"), "ActiveSupport.cast_boolean(flag)");
}

#[test]
fn key_conversions_ground_by_the_receiver_keys() {
    assert_eq!(emit("{ a: 1 }.stringify_keys"), "ActiveSupport.stringify_keys({ a: 1 })");
    assert_eq!(emit("{ \"a\" => 1 }.stringify_keys"), "{ \"a\" => 1 }");
    assert_eq!(emit("{ a: \"x\" }.deep_symbolize_keys"), "{ a: \"x\" }");
    assert_eq!(emit("{ \"a\" => \"x\" }.deep_symbolize_keys"), "ActiveSupport.symbolize_keys({ \"a\" => \"x\" })");
}

#[test]
fn a_deep_conversion_over_nested_hashes_is_left_alone() {
    let out = emit("{ a: { b: 1 } }.deep_symbolize_keys");
    assert!(out.ends_with(".deep_symbolize_keys"), "{out}");
}

#[test]
fn array_wrap_folds_one_shape_and_branches_a_union() {
    // A closed shape folds. `flag ? [1] : nil` is `Array | Nil`, so
    // one shape would wrap nil or nest the array. The fold branches
    // after binding the argument once.
    assert_eq!(emit("Array.wrap(nil)"), "[]");
    assert_eq!(emit("Array.wrap([1])"), "[1]");
    assert_eq!(emit("Array.wrap(\"solo\")"), "[\"solo\"]");
    let mixed = emit("Array.wrap(flag ? [1] : nil)");
    assert!(
        mixed.contains("nil?") && mixed.contains("__array_wrap"),
        "a union branches after one binding; got: {mixed}"
    );
    assert!(
        !mixed.contains("Array.wrap"),
        "the union is folded, not left as a call the ruby emit cannot run; got: {mixed}"
    );
}

#[test]
fn cast_boolean_answers_like_activemodel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/as_hash_boolean_runtime.rb"))
        .arg(root.join("runtime/ruby/active_support_ext.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ALL OK") && out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
