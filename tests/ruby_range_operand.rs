//! A range operand of an infix or boolean operator keeps its parentheses.
//!
//! `..`/`...` bind looser than every infix operator and than `&&`/`||`.
//! `out << (start...limit)` written bare is `(out << start)...limit`: it
//! pushes the Integer, and `ruby -w` reports "possibly useless use of ...
//! in void context", which a warnings-as-errors handler raises on when
//! the file loads. `(a..b) == r` bare is `a..(b == r)`, and
//! `r || (a..b)` bare is `(r || a)..b`.

use std::process::Command;

use roundhouse::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode};
use roundhouse::ident::{Symbol, VarId};
use roundhouse::span::Span;

fn node(n: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), n)
}

fn var(name: &str) -> Expr {
    node(ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

fn range(b: &str, e: &str, exclusive: bool) -> Expr {
    node(ExprNode::Range { begin: Some(var(b)), end: Some(var(e)), exclusive })
}

fn binop(l: Expr, op: &str, r: Expr) -> Expr {
    node(ExprNode::Send { recv: Some(l), method: Symbol::from(op), args: vec![r], block: None, parenthesized: false })
}

fn emit(e: &Expr) -> String {
    roundhouse::emit::ruby::emit_expr(e)
}

#[test]
fn a_range_argument_of_an_infix_operator_is_parenthesized() {
    assert_eq!(emit(&binop(var("out"), "<<", range("a", "b", true))), "out << (a...b)");
    assert_eq!(emit(&binop(var("out"), "<<", range("a", "b", false))), "out << (a..b)");
    assert_eq!(emit(&binop(var("r"), "==", range("a", "b", false))), "r == (a..b)");
    assert_eq!(emit(&binop(var("x"), "!~", range("a", "b", false))), "x !~ (a..b)");
}

#[test]
fn a_range_receiver_of_an_infix_operator_is_parenthesized() {
    assert_eq!(emit(&binop(range("a", "b", false), "==", var("r"))), "(a..b) == r");
    assert_eq!(emit(&binop(range("a", "b", false), "===", var("x"))), "(a..b) === x");
}

#[test]
fn a_range_operand_of_a_boolean_operator_is_parenthesized() {
    let or = node(ExprNode::BoolOp {
        op: BoolOpKind::Or,
        surface: BoolOpSurface::Symbol,
        left: var("r"),
        right: range("a", "b", false),
    });
    assert_eq!(emit(&or), "r || (a..b)");
}

#[test]
fn the_emitted_operands_evaluate_as_ruby_reads_the_source() {
    let push = emit(&binop(var("out"), "<<", range("a", "b", true)));
    let cmp = emit(&binop(range("a", "b", false), "==", var("r")));
    let script = format!(
        "a = 1; b = 3; r = (1..3); out = []\n{push}\np [out, {cmp}]\n"
    );
    let out = Command::new("ruby").arg("-w").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{script}\n{stderr}");
    assert!(!stderr.contains("void context"), "{script}\n{stderr}");
    assert_eq!(stdout.trim(), "[[1...3], true]", "{script}\n{stderr}");
}
