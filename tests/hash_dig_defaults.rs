use std::{collections::HashMap, path::PathBuf};

fn return_type(body: &str, declarations: &str) -> String {
    let source = format!(
        "{declarations}\nmodule ContainerProbe\n def self.value(raw)\n {body}\n end\nend\n"
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(HashMap::from([
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\nend\n".to_vec(),
        ),
        (PathBuf::from("lib/container_probe.rb"), source.into_bytes()),
    ]))
    .unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    rbs.lines()
        .find(|line| line.contains("def self.value:"))
        .and_then(|line| line.rsplit_once(" -> "))
        .unwrap_or_else(|| panic!("{rbs}"))
        .1
        .to_owned()
}

#[test]
fn literal_hash_and_plain_alias_keep_the_value_bound() {
    for body in [
        r#"{"k" => "ok"}.dig(JSON.parse(raw)) || "fallback""#,
        r#"h = {"k" => "ok"}; alias_hash = h; alias_hash.dig(JSON.parse(raw)) || "fallback""#,
    ] {
        assert_eq!(return_type(body, ""), "String");
    }
}

#[test]
fn changing_a_default_through_a_rebound_alias_does_not_prove_nil() {
    let actual = return_type(
        r#"h = {"k" => "ok"}; a = h; a.default = 42; a = {}; h.dig("missing")"#,
        "",
    );
    assert!(
        actual == "untyped" || actual.contains("Integer"),
        "{actual}"
    );
}

#[test]
fn storing_an_alias_in_a_container_does_not_prove_nil() {
    let actual = return_type(
        r#"h = {"k" => "ok"}; aliases = [h]; aliases.first.default = 42; h.dig("missing")"#,
        "",
    );
    assert!(
        actual == "untyped" || actual.contains("Integer"),
        "{actual}"
    );
}

#[test]
fn an_unrelated_prepend_preserves_literal_hash_precision() {
    assert_eq!(
        return_type(
            r#"{"k" => "ok"}.dig(JSON.parse(raw)) || "fallback""#,
            "module OtherHashMethods\n def other; 1; end\nend\nclass Hash\n prepend OtherHashMethods\nend"
        ),
        "String"
    );
}

#[test]
fn unresolved_prepend_does_not_prove_builtin_dig() {
    assert_eq!(
        return_type(
            r#"["ok"].dig(0)"#,
            "class Array\n prepend UnresolvedMethods\nend"
        ),
        "untyped"
    );
}
