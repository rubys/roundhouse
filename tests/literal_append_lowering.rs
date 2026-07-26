//! `"literal" << x` → `"literal" + x` (`lower::apply_literal_append_lowering`).
//!
//! Appending to a string literal is a Ruby idiom for splitting a long
//! string across lines — lobsters' StoryRepository#top builds a SQL
//! fragment that way. CRuby allows it (a literal without
//! `# frozen_string_literal` is a fresh mutable String); spinel freezes
//! string literals, so the same source raises FrozenError at run time and
//! took down `/top`. Nothing can observe the mutation, so `+` is
//! equivalent and frozen-safe.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_literal_append_lowering;

fn model_body(src: &str) -> String {
    let tree = vec![(std::path::PathBuf::from("app/models/thing.rb"), src.as_bytes().to_vec())]
        .into_iter()
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    apply_literal_append_lowering(&mut app);
    let m = app
        .models
        .iter()
        .chain(std::iter::empty())
        .find(|m| m.name.0.as_str() == "Thing");
    match m {
        Some(m) => format!("{:?}", m.body),
        None => {
            let c = app
                .library_classes
                .iter()
                .find(|c| c.name.0.as_str() == "Thing")
                .expect("Thing ingested");
            format!("{:?}", c.methods)
        }
    }
}

const WRAP: &str = "class Thing < ApplicationRecord\n  def q\n    %s\n  end\nend\n";

fn lowered(expr: &str) -> String {
    model_body(&WRAP.replace("%s", expr))
}

#[test]
fn literal_receiver_with_string_arg_becomes_plus() {
    let body = lowered(r#""a" << "b""#);
    assert!(body.contains("\"+\""), "expected + :\n{body}");
    assert!(!body.contains("\"<<\""), "<< should be gone:\n{body}");
}

#[test]
fn interpolated_argument_is_the_real_shape() {
    // lobsters: `"created_at >= (DATETIME('now', '- " << "#{dur} #{intv}'))"`
    let body = lowered(r##""sql " << "#{x} tail""##);
    assert!(body.contains("\"+\""), "expected + :\n{body}");
}

#[test]
fn non_literal_receiver_is_left_alone() {
    // An lvalue receiver's mutation IS observable through the name; that
    // case belongs to the `<lv>.sub!` → reassignment rewrite instead.
    let body = lowered("buf << \"b\"");
    assert!(body.contains("\"<<\""), "lvalue receiver must keep << :\n{body}");
}

#[test]
fn integer_argument_is_left_alone() {
    // `"a" << 65` appends a CODEPOINT ("aA"); `+` would TypeError. A
    // silently wrong rewrite here would be worse than the FrozenError.
    let body = lowered(r#""a" << 65"#);
    assert!(body.contains("\"<<\""), "codepoint append must keep << :\n{body}");
}

#[test]
fn array_literal_receiver_is_left_alone() {
    // Only STRING literals are frozen-temporary; `[] << x` is a normal
    // accumulator and must not become `[] + x`.
    let body = lowered(r#"[] << "b""#);
    assert!(body.contains("\"<<\""), "array push must keep << :\n{body}");
}
