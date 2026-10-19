//! `stub_const(Owner, :NAME, value) { … }` → a stack the reads consult.
//!
//! Rails' `ActiveSupport::Testing::ConstantStubbing#stub_const` replaces
//! the constant for the block (`remove_const` / `const_set`) and puts it
//! back after, so code that reads it at call time sees the stub. A
//! compiled target resolves constants at build time and has no
//! `const_set`, so there the call had no meaning at all: the spinel lane
//! refused the whole test file at its first `stub_const`.
//!
//! The constants a test stubs are known at compile time — every site
//! names its owner and its name as literals — so the rewrite is static.
//! Each stubbed constant keeps its definition and gains a sibling,
//! `NAME_STUB_STACK = [Owner::NAME]`; every read of `Owner::NAME` becomes
//! `Owner::NAME_STUB_STACK.last`, and the call becomes
//!
//! ```text
//! Owner::NAME_STUB_STACK.push(value)
//! begin
//!   <block body>
//! ensure
//!   Owner::NAME_STUB_STACK.pop
//! end
//! ```
//!
//! which is Rails' replace-and-restore, nested blocks and a raising
//! block included. Reads are matched on the analyzer-qualified path, so
//! a bare `NAME` inside its owner and `Owner::NAME` elsewhere are the
//! same read by the time this runs.
//!
//! What it does not reach: a REFLECTIVE read (`const_get(:NAME)`), which
//! has no path to match, and a stub that changes a scalar constant's
//! type (`TIME_LIMIT = 10` stubbed with `0.2`), which no one-typed stack
//! holds. Those sites, and any whose owner or constant this pass cannot
//! find, are left as written: on CRuby the test-support helper answers
//! them, and the spinel harness's helper fails the test saying why.

use std::collections::HashSet;

use crate::app::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// The sibling constant that holds a stubbed constant's value stack.
fn stack_name(name: &str) -> String {
    format!("{name}_STUB_STACK")
}

pub fn apply_stub_const_lowering(app: &mut App) {
    let mut wanted: Vec<(Vec<Symbol>, Option<Ty>)> = Vec::new();
    for tm in &app.test_modules {
        let mut visit = |e: &Expr| collect(e, &mut wanted);
        if let Some(setup) = &tm.setup {
            visit(setup);
        }
        for t in &tm.tests {
            visit(&t.body);
        }
        for m in &tm.helpers {
            visit(&m.body);
        }
    }
    if wanted.is_empty() {
        return;
    }
    // A stub that changes a scalar constant's type (`TIME_LIMIT = 10`
    // stubbed with `0.2`) has no single-typed stack to live in; every
    // site of that constant is left to the harness helper.
    let mut retyped: HashSet<Vec<Symbol>> = HashSet::new();
    for (path, ty) in &wanted {
        if let (Some(stub), Some(own)) = (ty, constant_ty(app, path)) {
            if is_scalar(stub) && is_scalar(&own) && *stub != own {
                retyped.insert(path.clone());
            }
        }
    }
    let mut stubbed: HashSet<Vec<Symbol>> = HashSet::new();
    for (path, _) in wanted {
        if !stubbed.contains(&path) && !retyped.contains(&path) && insert_stack(app, &path) {
            stubbed.insert(path);
        }
    }
    if stubbed.is_empty() {
        return;
    }
    crate::lower::for_each_forwarding_body(app, &mut |e| rewrite(e, &stubbed));
}

/// `stub_const(Const, :NAME, value) { … }` with a literal owner and
/// name: the constant's full path.
fn stub_site(expr: &Expr) -> Option<Vec<Symbol>> {
    let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "stub_const" || args.len() != 3 {
        return None;
    }
    if !matches!(&*block.node, ExprNode::Lambda { params, .. } if params.is_empty()) {
        return None;
    }
    let ExprNode::Const { path: owner } = &*args[0].node else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*args[1].node else { return None };
    let mut path: Vec<Symbol> = owner
        .iter()
        .map(|s| Symbol::from(s.as_str().trim_start_matches("::")))
        .collect();
    path.push(name.clone());
    Some(path)
}

fn collect(expr: &Expr, out: &mut Vec<(Vec<Symbol>, Option<Ty>)>) {
    if let Some(path) = stub_site(expr) {
        let ExprNode::Send { args, .. } = &*expr.node else { unreachable!() };
        out.push((path, args[2].ty.clone()));
    }
    expr.node.for_each_child(&mut |c| collect(c, out));
}

fn is_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym)
}

/// The analyzed type of the constant at `path`'s definition.
fn constant_ty(app: &App, path: &[Symbol]) -> Option<Ty> {
    let (name, owner) = path.split_last()?;
    let owner_name = owner.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let assigned = |expr: &Expr| match &*expr.node {
        ExprNode::Assign { target: LValue::Const { path }, value } if path.last() == Some(name) => value.ty.clone(),
        _ => None,
    };
    for lc in app.library_classes.iter().filter(|lc| lc.name.0.as_str() == owner_name) {
        if let Some((_, value)) = lc.constants.iter().find(|(n, _)| n == name) {
            return value.ty.clone();
        }
    }
    for controller in app.controllers.iter().filter(|c| c.name.0.as_str() == owner_name) {
        for item in &controller.body {
            if let ControllerBodyItem::Unknown { expr, .. } = item {
                if defines(expr, name) {
                    return assigned(expr);
                }
            }
        }
    }
    for model in app.models.iter().filter(|m| m.name.0.as_str() == owner_name) {
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                if defines(expr, name) {
                    return assigned(expr);
                }
            }
        }
    }
    None
}

/// Give the constant at `path` its stack sibling, next to its
/// definition. False when no class this app defines carries it.
fn insert_stack(app: &mut App, path: &[Symbol]) -> bool {
    let (name, owner) = path.split_last().expect("a stubbed path names a constant");
    let owner_name = owner.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let stack = Symbol::from(stack_name(name.as_str()));

    for lc in &mut app.library_classes {
        if lc.name.0.as_str() != owner_name {
            continue;
        }
        if let Some(i) = lc.constants.iter().position(|(n, _)| n == name) {
            let value = at_span(stack_value(path), lc.constants[i].1.span);
            lc.constants.insert(i + 1, (stack, value));
            return true;
        }
    }
    for controller in &mut app.controllers {
        if controller.name.0.as_str() != owner_name {
            continue;
        }
        let at = controller.body.iter().position(|item| {
            matches!(item, ControllerBodyItem::Unknown { expr, .. } if defines(expr, name))
        });
        if let Some(i) = at {
            let span = match &controller.body[i] {
                ControllerBodyItem::Unknown { expr, .. } => value_span(expr),
                _ => unreachable!(),
            };
            let expr = at_span(define(&stack, at_span(stack_value(path), span)), span);
            controller.body.insert(
                i + 1,
                ControllerBodyItem::Unknown { expr, leading_comments: Vec::new(), leading_blank_line: false },
            );
            return true;
        }
    }
    for model in &mut app.models {
        if model.name.0.as_str() != owner_name {
            continue;
        }
        let at = model.body.iter().position(|item| {
            matches!(item, ModelBodyItem::Unknown { expr, .. } if defines(expr, name))
        });
        if let Some(i) = at {
            let span = match &model.body[i] {
                ModelBodyItem::Unknown { expr, .. } => value_span(expr),
                _ => unreachable!(),
            };
            let expr = at_span(define(&stack, at_span(stack_value(path), span)), span);
            model.body.insert(
                i + 1,
                ModelBodyItem::Unknown { expr, leading_comments: Vec::new(), leading_blank_line: false },
            );
            return true;
        }
    }
    false
}

/// Whether a class-body statement is `NAME = …`.
fn defines(expr: &Expr, name: &Symbol) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Assign { target: LValue::Const { path }, .. } if path.last() == Some(name)
    )
}

/// The span of a `NAME = value` statement's value, which is what a
/// class body's constants are ordered by.
fn value_span(statement: &Expr) -> Span {
    match &*statement.node {
        ExprNode::Assign { value, .. } => value.span,
        _ => statement.span,
    }
}

fn define(name: &Symbol, value: Expr) -> Expr {
    expr(ExprNode::Assign { target: LValue::Const { path: vec![name.clone()] }, value })
}

/// `[Owner::NAME]` — the stack starts holding the constant's own value.
fn stack_value(path: &[Symbol]) -> Expr {
    expr(ExprNode::Array {
        elements: vec![expr(ExprNode::Const { path: path.to_vec() })],
        style: ArrayStyle::Brackets,
    })
}

fn stack_const(path: &[Symbol]) -> Expr {
    let (name, owner) = path.split_last().expect("a stubbed path names a constant");
    let mut stack = owner.to_vec();
    stack.push(Symbol::from(stack_name(name.as_str())));
    expr(ExprNode::Const { path: stack })
}

fn send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    let parenthesized = !args.is_empty();
    expr(ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized })
}

fn expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

/// The emitter orders a class body by source position, so the stack
/// takes its constant's span and lands right after it (the sort is
/// stable, and the stack is inserted after).
fn at_span(mut e: Expr, span: Span) -> Expr {
    e.span = span;
    e
}

fn rewrite(e: &mut Expr, stubbed: &HashSet<Vec<Symbol>>) {
    // The stack's own initializer (`[Owner::NAME]`, whose read is
    // synthetic) reads the constant it shadows; that read must stay one.
    if let ExprNode::Array { elements, .. } = &*e.node {
        if let [only] = &elements[..] {
            if let ExprNode::Const { path } = &*only.node {
                if stubbed.contains(path) && only.span == Span::synthetic() {
                    return;
                }
            }
        }
    }
    if let Some(path) = stub_site(e).filter(|p| stubbed.contains(p)) {
        let ExprNode::Send { args, block: Some(block), .. } = &mut *e.node else { unreachable!() };
        let mut value = args.remove(2);
        rewrite(&mut value, stubbed);
        let ExprNode::Lambda { body, .. } = &mut *block.node else { unreachable!() };
        let mut body = std::mem::replace(body, expr(ExprNode::Lit { value: Literal::Nil }));
        rewrite(&mut body, stubbed);
        let push = send(stack_const(&path), "push", vec![value]);
        let pop = send(stack_const(&path), "pop", Vec::new());
        let guarded = expr(ExprNode::BeginRescue {
            body,
            rescues: Vec::new(),
            else_branch: None,
            ensure: Some(pop),
            implicit: false,
        });
        *e = Expr::new(e.span, ExprNode::Seq { exprs: vec![push, guarded] });
        return;
    }
    if let ExprNode::Const { path } = &*e.node {
        if stubbed.contains(path) {
            let span = e.span;
            *e = send(stack_const(path), "last", Vec::new());
            e.span = span;
        }
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, stubbed));
}
