//! `-x` / `+x` on a number (`lower::numeric_unary`), and a negative
//! literal or infix expression as a method receiver.
//!
//! Prism folds `-5` into a literal, but `-a` on a local is a send of
//! `-@`. No non-Ruby emitter has a prefix operator for that name, so
//! each wrote a method call: `a.-@` in TypeScript and Python, `a.-@()`
//! in Rust, none of which parse. The receiver half is precedence:
//! `(-5).abs` came out as `-5.abs`, which TypeScript and Python reject
//! and Rust reads as `-(5_i64.abs())`, answering -5 instead of 5.
//! Rust's half is pinned next to its emitter (`emit/rust/library.rs`).
use roundhouse::analyze::Analyzer;
use roundhouse::emit;
use roundhouse::lower::numeric_unary::apply_numeric_unary_lowering;
use roundhouse::project::BuildTarget;
use std::collections::HashMap;
use std::path::PathBuf;

const SOURCE: &str = r#"class UnaryProbe
  def neg_int
    a = 7
    -a
  end
  def neg_float
    f = 2.5
    -f
  end
  def pos_int
    a = 7
    +a
  end
  def neg_sum
    a = 3
    -(a + 4)
  end
  def neg_receiver
    (-5).abs
  end
  def infix_receiver
    a = 3
    (a - 10).abs
  end
end
"#;

// A receiver that defines `-@` itself keeps the unary send.
const CUSTOM: &str = r#"class UnaryVec
  def initialize(x)
    @x = x
  end
  def -@
    UnaryVec.new(0)
  end
  def flipped
    v = UnaryVec.new(1)
    -v
  end
end
"#;

fn lowered() -> roundhouse::App {
    let mut app = roundhouse::ingest::ingest_app_from_tree(HashMap::from([
        (PathBuf::from("app/lib/unary_probe.rb"), SOURCE.as_bytes().to_vec()),
        (PathBuf::from("app/lib/unary_vec.rb"), CUSTOM.as_bytes().to_vec()),
    ]))
    .unwrap();
    Analyzer::new(&app).analyze(&mut app);
    apply_numeric_unary_lowering(&mut app);
    app
}

fn body<'a>(app: &'a roundhouse::App, name: &str) -> &'a roundhouse::expr::Expr {
    method_body(app, "UnaryProbe", name)
}

fn method_body<'a>(app: &'a roundhouse::App, class: &str, name: &str) -> &'a roundhouse::expr::Expr {
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == class)
        .unwrap();
    &class.methods.iter().find(|m| m.name.as_str() == name).unwrap().body
}

fn project(target: BuildTarget) -> String {
    let app = lowered();
    let files = roundhouse::project::target_files(&app, std::path::Path::new("."), target).unwrap();
    files
        .into_iter()
        .filter(|(path, _)| path.contains("unary_probe"))
        .map(|(_, content)| content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn unary_on_a_number_lowers_to_arithmetic() {
    let app = lowered();
    let neg = format!("{:?}", body(&app, "neg_int"));
    assert!(!neg.contains("-@"), "`-a` should be lowered:\n{neg}");
    assert!(neg.contains("\"*\""), "`-a` should become `a * -1`:\n{neg}");
    let pos = format!("{:?}", body(&app, "pos_int"));
    assert!(!pos.contains("+@"), "`+a` should be the receiver itself:\n{pos}");
}

#[test]
fn unary_on_a_custom_receiver_is_left_alone() {
    let app = lowered();
    let flipped = format!("{:?}", method_body(&app, "UnaryVec", "flipped"));
    assert!(flipped.contains("-@"), "custom `-@` should stay a send:\n{flipped}");
    assert!(!flipped.contains("\"*\""), "custom `-@` should not become `* -1`:\n{flipped}");
}

#[test]
fn typescript_spells_negation_and_parenthesizes_receivers() {
    let out = project(BuildTarget::Typescript);
    assert!(!out.contains("-@"), "method-call spelling leaked:\n{out}");
    assert!(out.contains("a * -1"), "{out}");
    assert!(out.contains("f * -1"), "{out}");
    assert!(out.contains("(a + 4) * -1"), "{out}");
    assert!(out.contains("(-5)."), "negative receiver lost its parens:\n{out}");
    assert!(out.contains("(a - 10)."), "infix receiver lost its parens:\n{out}");
}

#[test]
fn python_spells_negation_and_parenthesizes_receivers() {
    let app = lowered();
    let py = |name: &str| emit::python::emit_expr_for_runtime(body(&app, name));
    assert!(py("neg_int").contains("a * -1"), "{}", py("neg_int"));
    assert!(!py("pos_int").contains("+@"), "{}", py("pos_int"));
    // `a + 4 * -1` would be -1, not -7.
    assert!(py("neg_sum").contains("(a + 4) * -1"), "{}", py("neg_sum"));
    assert!(py("neg_receiver").contains("(-5)."), "{}", py("neg_receiver"));
    assert!(py("infix_receiver").contains("(a - 10)."), "{}", py("infix_receiver"));
}
