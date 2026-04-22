//! RBS parser smoke test: the ruby-rbs crate builds on this machine
//! and parses signatures end-to-end, producing a typed AST we can walk.

use ruby_rbs::node::{Node, parse};

#[test]
fn parse_method_signature() {
    let src = "class Pluralizer\n  def pluralize: (Integer, String) -> String\nend\n";
    let sig = parse(src).expect("valid RBS parses");

    let decl = sig.declarations().iter().next().expect("one declaration");
    let Node::Class(class) = decl else {
        panic!("expected Class declaration");
    };
    assert_eq!(class.name().name().as_str(), "Pluralizer");

    let member = class.members().iter().next().expect("one member");
    let Node::MethodDefinition(method) = member else {
        panic!("expected MethodDefinition");
    };
    assert_eq!(method.name().as_str(), "pluralize");
}

#[test]
fn parse_error_is_reported() {
    let err = parse("class { end").expect_err("broken RBS is rejected");
    assert!(!err.is_empty(), "parser surfaces an error message");
}
