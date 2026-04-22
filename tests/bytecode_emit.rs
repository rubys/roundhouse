//! M3a: IR → bytecode walker.
//!
//! Hand-constructs typed `Expr` trees (skipping ingest + analyze, which
//! is tested elsewhere), walks them through the bytecode emitter, and
//! runs the resulting bytecode through the M2 VM to verify end-to-end
//! IR → bytecode → execution correctness.
//!
//! The walker itself is the unit under test; the VM is reused as the
//! oracle for "did the emitted code do the right thing?"
//!
//! Scope per M3a: Lit, Var, Let, Seq, If, Assign-to-Var, and Send for
//! typed integer arithmetic + comparisons. BoolOp, string concat,
//! user-method Send, collections, Lambda/Apply, etc. return
//! NotYetSupported and are verified as such.

use roundhouse::bytecode::{Value, Vm, WalkError, Walker};
use roundhouse::expr::{Expr, ExprNode, LValue, Literal};
use roundhouse::ident::{Symbol, VarId};
use roundhouse::span::Span;
use roundhouse::ty::Ty;

// ── Expr construction helpers ─────────────────────────────────────

fn typed(node: ExprNode, ty: Ty) -> Expr {
    let mut e = Expr::new(Span::synthetic(), node);
    e.ty = Some(ty);
    e
}

fn untyped(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn lit_int(value: i64) -> Expr {
    typed(
        ExprNode::Lit {
            value: Literal::Int { value },
        },
        Ty::Int,
    )
}

fn lit_bool(value: bool) -> Expr {
    typed(
        ExprNode::Lit {
            value: Literal::Bool { value },
        },
        Ty::Bool,
    )
}

fn lit_str(value: &str) -> Expr {
    typed(
        ExprNode::Lit {
            value: Literal::Str {
                value: value.into(),
            },
        },
        Ty::Str,
    )
}

fn lit_nil() -> Expr {
    typed(
        ExprNode::Lit {
            value: Literal::Nil,
        },
        Ty::Nil,
    )
}

fn var(id: u32, name: &str, ty: Ty) -> Expr {
    // In the real pipeline, analyze populates every Var's type before
    // the walker runs. Send's receiver check relies on that — so
    // test helpers set it explicitly to mirror that invariant.
    typed(
        ExprNode::Var {
            id: VarId(id),
            name: Symbol::from(name),
        },
        ty,
    )
}

fn send_i64(recv: Expr, method: &str, arg: Expr, result_ty: Ty) -> Expr {
    typed(
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
        result_ty,
    )
}

fn assign_var(id: u32, name: &str, value: Expr) -> Expr {
    untyped(ExprNode::Assign {
        target: LValue::Var {
            id: VarId(id),
            name: Symbol::from(name),
        },
        value,
    })
}

fn let_in(id: u32, name: &str, value: Expr, body: Expr) -> Expr {
    untyped(ExprNode::Let {
        id: VarId(id),
        name: Symbol::from(name),
        value,
        body,
    })
}

fn seq(exprs: Vec<Expr>) -> Expr {
    untyped(ExprNode::Seq { exprs })
}

fn if_expr(cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
    untyped(ExprNode::If {
        cond,
        then_branch,
        else_branch,
    })
}

// ── Run helper: walk, then execute via VM ─────────────────────────

fn run(expr: &Expr) -> Result<Option<Value>, String> {
    let mut walker = Walker::new();
    walker.walk(expr).map_err(|e| format!("{:?}", e))?;
    let locals = walker.locals_count() as usize;
    let program = walker.into_program();
    let mut vm = Vm::new(&program).with_locals(locals);
    vm.run().map_err(|e| format!("{:?}", e))
}

// ── Literal emission ─────────────────────────────────────────────

#[test]
fn lit_int_runs() {
    assert_eq!(run(&lit_int(42)).unwrap(), Some(Value::Int(42)));
}

#[test]
fn lit_bool_runs() {
    assert_eq!(run(&lit_bool(true)).unwrap(), Some(Value::Bool(true)));
}

#[test]
fn lit_nil_runs() {
    assert_eq!(run(&lit_nil()).unwrap(), Some(Value::Nil));
}

#[test]
fn lit_str_pooled_and_runs() {
    // Verify it runs
    assert_eq!(
        run(&lit_str("hello")).unwrap(),
        Some(Value::Str("hello".into()))
    );
    // And that duplicate strings intern to the same slot.
    let mut w = Walker::new();
    w.walk(&lit_str("x")).unwrap();
    w.walk(&lit_str("x")).unwrap();
    let p = w.into_program();
    assert_eq!(p.string_pool.len(), 1);
}

// ── Arithmetic via Send ──────────────────────────────────────────

#[test]
fn arithmetic_precedence_via_nested_sends() {
    // (42 + 8) * 2 = 100
    // Built as: (42 + 8).*(2) — Send{Send{42, +, 8}, *, 2}
    let inner = send_i64(lit_int(42), "+", lit_int(8), Ty::Int);
    let outer = send_i64(inner, "*", lit_int(2), Ty::Int);
    assert_eq!(run(&outer).unwrap(), Some(Value::Int(100)));
}

#[test]
fn subtraction_respects_operand_order() {
    // 10 - 3 = 7
    let expr = send_i64(lit_int(10), "-", lit_int(3), Ty::Int);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(7)));
}

#[test]
fn division_runs() {
    // 20 / 4 = 5
    let expr = send_i64(lit_int(20), "/", lit_int(4), Ty::Int);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(5)));
}

// ── Integer comparisons via Send ─────────────────────────────────

#[test]
fn gt_comparison_true() {
    let expr = send_i64(lit_int(5), ">", lit_int(3), Ty::Bool);
    assert_eq!(run(&expr).unwrap(), Some(Value::Bool(true)));
}

#[test]
fn eq_comparison_false() {
    let expr = send_i64(lit_int(5), "==", lit_int(3), Ty::Bool);
    assert_eq!(run(&expr).unwrap(), Some(Value::Bool(false)));
}

#[test]
fn all_comparisons_emit_correct_ops() {
    let cases: &[(&str, i64, i64, bool)] = &[
        ("==", 5, 5, true),
        ("!=", 5, 6, true),
        ("<", 5, 6, true),
        ("<=", 5, 5, true),
        (">", 6, 5, true),
        (">=", 5, 5, true),
    ];
    for (method, a, b, expected) in cases {
        let expr = send_i64(lit_int(*a), method, lit_int(*b), Ty::Bool);
        assert_eq!(
            run(&expr).unwrap(),
            Some(Value::Bool(*expected)),
            "{} {} {} should be {}",
            a,
            method,
            b,
            expected
        );
    }
}

// ── Let + Var ────────────────────────────────────────────────────

#[test]
fn let_binds_and_body_reads() {
    // let x = 10 in x + 5
    let body = send_i64(var(1, "x", Ty::Int), "+", lit_int(5), Ty::Int);
    let expr = let_in(1, "x", lit_int(10), body);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(15)));
}

#[test]
fn nested_lets_each_get_their_own_slot() {
    // let x = 10 in let y = 20 in x + y
    let inner_body = send_i64(var(1, "x", Ty::Int), "+", var(2, "y", Ty::Int), Ty::Int);
    let inner_let = let_in(2, "y", lit_int(20), inner_body);
    let outer_let = let_in(1, "x", lit_int(10), inner_let);
    assert_eq!(run(&outer_let).unwrap(), Some(Value::Int(30)));
}

#[test]
fn unbound_var_errors() {
    // Walking Var without a prior binding is a WalkError.
    let expr = var(99, "ghost", Ty::Int);
    let mut w = Walker::new();
    assert_eq!(w.walk(&expr), Err(WalkError::UnboundVariable(VarId(99))));
}

// ── Seq ──────────────────────────────────────────────────────────

#[test]
fn seq_returns_last_value() {
    // [1, 2, 3] => 3
    let expr = seq(vec![lit_int(1), lit_int(2), lit_int(3)]);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(3)));
}

#[test]
fn seq_pops_intermediate_values() {
    // A Seq with 5 elements leaves exactly one value on the stack.
    let expr = seq(vec![
        lit_int(1),
        lit_int(2),
        lit_int(3),
        lit_int(4),
        lit_int(5),
    ]);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(5)));
}

#[test]
fn empty_seq_returns_nil() {
    let expr = seq(vec![]);
    assert_eq!(run(&expr).unwrap(), Some(Value::Nil));
}

// ── If ───────────────────────────────────────────────────────────

#[test]
fn if_true_branch_selects_then() {
    // if (5 > 3) then 100 else 200
    let cond = send_i64(lit_int(5), ">", lit_int(3), Ty::Bool);
    let expr = if_expr(cond, lit_int(100), lit_int(200));
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(100)));
}

#[test]
fn if_false_branch_selects_else() {
    let cond = send_i64(lit_int(3), ">", lit_int(5), Ty::Bool);
    let expr = if_expr(cond, lit_int(100), lit_int(200));
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(200)));
}

#[test]
fn nested_if_patches_correctly() {
    // if true then (if false then 1 else 2) else 3
    // Expected: 2
    let inner = if_expr(lit_bool(false), lit_int(1), lit_int(2));
    let outer = if_expr(lit_bool(true), inner, lit_int(3));
    assert_eq!(run(&outer).unwrap(), Some(Value::Int(2)));
}

#[test]
fn if_branches_with_nontrivial_bodies() {
    // if (2 > 1) then (10 + 5) else (20 - 5)
    // Expected: 15
    let cond = send_i64(lit_int(2), ">", lit_int(1), Ty::Bool);
    let then_br = send_i64(lit_int(10), "+", lit_int(5), Ty::Int);
    let else_br = send_i64(lit_int(20), "-", lit_int(5), Ty::Int);
    let expr = if_expr(cond, then_br, else_br);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(15)));
}

// ── Assign ───────────────────────────────────────────────────────

#[test]
fn assign_leaves_value_on_stack() {
    // x = 5  (evaluates to 5, and x now bound to 5)
    let expr = assign_var(1, "x", lit_int(5));
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(5)));
}

#[test]
fn assign_then_read_via_seq() {
    // x = 5; x + 1  => 6
    let assign = assign_var(1, "x", lit_int(5));
    let read = send_i64(var(1, "x", Ty::Int), "+", lit_int(1), Ty::Int);
    let expr = seq(vec![assign, read]);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(6)));
}

#[test]
fn reassign_updates_existing_slot() {
    // x = 5; x = 10; x   => 10
    let expr = seq(vec![
        assign_var(1, "x", lit_int(5)),
        assign_var(1, "x", lit_int(10)),
        var(1, "x", Ty::Int),
    ]);
    assert_eq!(run(&expr).unwrap(), Some(Value::Int(10)));
}

// ── Roundtrip: emitted Program survives serde ────────────────────

#[test]
fn emitted_program_roundtrips_through_serde() {
    let expr = send_i64(
        send_i64(lit_int(42), "+", lit_int(8), Ty::Int),
        "*",
        lit_int(2),
        Ty::Int,
    );
    let mut w = Walker::new();
    w.walk(&expr).unwrap();
    let locals = w.locals_count() as usize;
    let program = w.into_program();

    let json = serde_json::to_string(&program).unwrap();
    let program2: roundhouse::bytecode::Program = serde_json::from_str(&json).unwrap();
    assert_eq!(program, program2);

    // And the re-deserialized program still executes correctly.
    let mut vm = Vm::new(&program2).with_locals(locals);
    assert_eq!(vm.run().unwrap(), Some(Value::Int(100)));
}

// ── BoolOp short-circuit (M3b) ───────────────────────────────────

fn bool_and(left: Expr, right: Expr) -> Expr {
    use roundhouse::expr::{BoolOpKind, BoolOpSurface};
    typed(
        ExprNode::BoolOp {
            op: BoolOpKind::And,
            surface: BoolOpSurface::Symbol,
            left,
            right,
        },
        Ty::Bool,
    )
}

fn bool_or(left: Expr, right: Expr) -> Expr {
    use roundhouse::expr::{BoolOpKind, BoolOpSurface};
    typed(
        ExprNode::BoolOp {
            op: BoolOpKind::Or,
            surface: BoolOpSurface::Symbol,
            left,
            right,
        },
        Ty::Bool,
    )
}

#[test]
fn and_true_true_is_true() {
    assert_eq!(
        run(&bool_and(lit_bool(true), lit_bool(true))).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn and_true_false_is_false() {
    assert_eq!(
        run(&bool_and(lit_bool(true), lit_bool(false))).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn and_short_circuits_on_false_left() {
    // `false && (10 / 0 > 0)` — if the right side were evaluated we'd
    // get DivisionByZero from the VM. If short-circuit works, we
    // return false without touching the right side.
    let divide_by_zero = send_i64(lit_int(10), "/", lit_int(0), Ty::Int);
    let right = send_i64(divide_by_zero, ">", lit_int(0), Ty::Bool);
    let expr = bool_and(lit_bool(false), right);
    assert_eq!(run(&expr).unwrap(), Some(Value::Bool(false)));
}

#[test]
fn or_false_false_is_false() {
    assert_eq!(
        run(&bool_or(lit_bool(false), lit_bool(false))).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn or_true_false_is_true() {
    assert_eq!(
        run(&bool_or(lit_bool(true), lit_bool(false))).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn or_short_circuits_on_true_left() {
    // `true || (10 / 0 > 0)` — right side would DivisionByZero if
    // evaluated. Short-circuit returns true.
    let divide_by_zero = send_i64(lit_int(10), "/", lit_int(0), Ty::Int);
    let right = send_i64(divide_by_zero, ">", lit_int(0), Ty::Bool);
    let expr = bool_or(lit_bool(true), right);
    assert_eq!(run(&expr).unwrap(), Some(Value::Bool(true)));
}

#[test]
fn and_chains_correctly() {
    // true && (true && false) => false
    let inner = bool_and(lit_bool(true), lit_bool(false));
    let outer = bool_and(lit_bool(true), inner);
    assert_eq!(run(&outer).unwrap(), Some(Value::Bool(false)));
}

#[test]
fn bool_op_mixes_with_comparisons() {
    // (5 > 3) && (2 < 4) => true
    let left = send_i64(lit_int(5), ">", lit_int(3), Ty::Bool);
    let right = send_i64(lit_int(2), "<", lit_int(4), Ty::Bool);
    let expr = bool_and(left, right);
    assert_eq!(run(&expr).unwrap(), Some(Value::Bool(true)));
}

#[test]
fn bool_op_non_bool_operand_not_yet_supported() {
    // `true && 5` — right side isn't Bool-typed. This is the
    // "lift opportunity" signal — Ruby's truthy `&&` over mixed
    // types needs either a Truthy opcode or analyzer-produced
    // explicit casts, neither of which is M3b's job.
    let expr = bool_and(lit_bool(true), lit_int(5));
    let mut w = Walker::new();
    match w.walk(&expr) {
        Err(WalkError::NotYetSupported(msg)) => {
            assert!(msg.contains("non-Bool"), "message was: {}", msg)
        }
        other => panic!("expected NotYetSupported(non-Bool), got {:?}", other),
    }
}

// ── Deferred nodes / operations fail cleanly ─────────────────────

#[test]
fn send_on_non_int_receiver_not_yet_supported() {
    // "a" + "b" — string concat isn't in M3a's Send handler.
    let expr = send_i64(lit_str("a"), "+", lit_str("b"), Ty::Str);
    let mut w = Walker::new();
    assert!(matches!(w.walk(&expr), Err(WalkError::NotYetSupported(_))));
}

#[test]
fn send_with_unknown_int_method_not_yet_supported() {
    // 5.odd? — not in M3a's arithmetic/comparison table.
    let expr = typed(
        ExprNode::Send {
            recv: Some(lit_int(5)),
            method: Symbol::from("odd?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
        Ty::Bool,
    );
    let mut w = Walker::new();
    match w.walk(&expr) {
        Err(WalkError::NotYetSupported(msg)) => assert!(msg.contains("odd?")),
        other => panic!("expected NotYetSupported(odd?), got {:?}", other),
    }
}

#[test]
fn assign_to_ivar_not_yet_supported() {
    let expr = untyped(ExprNode::Assign {
        target: LValue::Ivar {
            name: Symbol::from("@x"),
        },
        value: lit_int(5),
    });
    let mut w = Walker::new();
    match w.walk(&expr) {
        Err(WalkError::NotYetSupported(msg)) => assert!(msg.contains("ivar")),
        other => panic!("expected NotYetSupported(ivar), got {:?}", other),
    }
}
