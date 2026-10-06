//! Wrap wrappable `class X < Array` into Object + `@elements`.
//!
//! Spinel (and any AOT that refuses core subclasses) cannot emit a real
//! Array subclass (`refuse_builtin_subclass`). Rewrite at ingest into an
//! Object that holds the records in `@elements` and exposes Array's
//! collection protocol. Same shape Spinel's refusal message asks for;
//! done here so every target sees one typed class. Size-based
//! `super(n)` / `super(n, fill)` keep the Array parent. This is not
//! Array identity (`is_a?(Array)` stays false) — Spinel #7584 is the
//! honest subclass path.

use std::collections::HashSet;

use crate::dialect::{MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;

/// If `parent` is `Array` / `::Array` and the initializer is wrappable,
/// clear the parent and return synthesized + rewritten methods.
/// Otherwise return the inputs unchanged (honest Array subclass path).
pub(super) fn wrap_array_subclass_if_applicable(
    owner: &ClassId,
    parent: Option<ClassId>,
    methods: Vec<MethodDef>,
) -> (Option<ClassId>, Vec<MethodDef>) {
    if !parent.as_ref().is_some_and(is_array_parent) {
        return (parent, methods);
    }
    match try_wrap_array_subclass(owner, methods) {
        Ok(wrapped) => (None, wrapped),
        Err(kept) => (parent, kept),
    }
}

fn is_array_parent(parent: &ClassId) -> bool {
    matches!(parent.0.as_str(), "Array" | "::Array")
}

/// How each protocol name forwards onto `@elements`.
enum Forward {
    /// `to_a` / `to_ary` — return `@elements` itself.
    Identity,
    Zero,
    One(&'static str),
    Splat,
    Enumerable,
    Each,
}

/// Single inventory driving both synthesis and pure-`super` drop.
const PROTOCOL: &[(&str, Forward)] = &[
    ("to_a", Forward::Identity),
    ("to_ary", Forward::Identity),
    ("each", Forward::Each),
    ("+", Forward::One("other")),
    ("any?", Forward::Enumerable),
    ("empty?", Forward::Zero),
    ("size", Forward::Zero),
    ("length", Forward::Zero),
    ("count", Forward::Enumerable),
    ("first", Forward::Splat),
    ("last", Forward::Splat),
    ("drop", Forward::One("n")),
    ("all?", Forward::Enumerable),
    ("map", Forward::Enumerable),
    ("select", Forward::Enumerable),
    ("include?", Forward::One("item")),
    ("[]", Forward::Splat),
];

fn protocol_names() -> HashSet<&'static str> {
    PROTOCOL.iter().map(|(name, _)| *name).collect()
}

/// Rewrite `class X < Array` into an Object that holds `@elements`.
///
/// Returns `Err(methods)` when the wrapper cannot represent the class
/// honestly — the caller keeps the Array parent.
///
/// Known ledger gap: `super(n)` where `n` is a variable size (not an
/// integer literal) is treated as a collection wrap (`@elements = n`).
/// Literal `super(3)` / `super(3, fill)` correctly keep the Array
/// parent. Distinguishing variable size from `super(records)` needs
/// types; until Spinel #7584, collection-shaped idents are the corpus.
fn try_wrap_array_subclass(
    owner: &ClassId,
    mut methods: Vec<MethodDef>,
) -> Result<Vec<MethodDef>, Vec<MethodDef>> {
    if !initialize_super_is_wrappable(&methods) {
        return Err(methods);
    }
    let synth_names = protocol_names();
    // Decorated/`return super` bodies on protocol names are not
    // rewritable; wrapping would clear Array and leave dead `super`.
    // Fail closed — keep the Array parent.
    if methods.iter().any(|m| {
        m.receiver == MethodReceiver::Instance
            && synth_names.contains(m.name.as_str())
            && !is_pure_super_body(&m.body)
            && body_contains_super(&m.body)
    }) {
        return Err(methods);
    }
    for method in &mut methods {
        if method.receiver == MethodReceiver::Instance && method.name.as_str() == "initialize" {
            let had_super = body_contains_super(&method.body);
            rewrite_array_super_to_elements(&mut method.body, &method.params);
            // Custom initialize that never calls super: MRI still gets an
            // empty Array from the parent. Seed `@elements = []` so
            // protocol methods do not call through nil.
            if !had_super {
                let span = method.body.span;
                let seed = Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Ivar {
                            name: Symbol::from("elements"),
                        },
                        value: Expr::new(
                            span,
                            ExprNode::Array {
                                elements: vec![],
                                style: crate::expr::ArrayStyle::default(),
                            },
                        ),
                    },
                );
                let old = std::mem::replace(&mut method.body, seed.clone());
                method.body = Expr::new(span, ExprNode::Seq { exprs: vec![seed, old] });
            }
        }
    }
    // Pure-`super` overrides of synthesized names would leave a dead
    // `super` after the Array parent is cleared. Drop them so the
    // splat/block forward wins.
    methods.retain(|m| {
        if m.receiver != MethodReceiver::Instance || !synth_names.contains(m.name.as_str()) {
            return true;
        }
        !is_pure_super_body(&m.body)
    });
    let existing: HashSet<String> = methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Instance)
        .map(|m| m.name.as_str().to_string())
        .collect();
    let mut synthesized = synth_array_wrapper_methods(owner);
    synthesized.retain(|m| !existing.contains(m.name.as_str()));
    if !existing.contains("initialize") {
        synthesized.push(synth_empty_array_initialize(owner));
    }
    synthesized.append(&mut methods);
    Ok(synthesized)
}

/// True when every `Super` in `initialize` is a form the wrapper can
/// express: bare `super` with a resolvable first positional, `super()`,
/// or `super(collection)` (one non-integer arg). Size forms and bare
/// `super` with no positional keep the Array parent (fail closed).
fn initialize_super_is_wrappable(methods: &[MethodDef]) -> bool {
    let Some(init) = methods
        .iter()
        .find(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "initialize")
    else {
        return true;
    };
    let first_positional = first_positional_param(&init.params);
    let mut ok = true;
    let mut visit = |e: &Expr| {
        match &*e.node {
            ExprNode::Super { args: Some(args) } => {
                if args.len() >= 2 {
                    ok = false;
                } else if args.len() == 1 && is_integer_lit(&args[0]) {
                    // `super(3)` is Array's size constructor, not a collection.
                    ok = false;
                }
            }
            // Bare `super` with no positional would leave a dead Super
            // after the Array parent is cleared — refuse the wrap.
            ExprNode::Super { args: None } if first_positional.is_none() => {
                ok = false;
            }
            _ => {}
        }
    };
    fn walk(expr: &Expr, visit: &mut dyn FnMut(&Expr)) {
        visit(expr);
        expr.node.for_each_child(&mut |c| walk(c, visit));
    }
    walk(&init.body, &mut visit);
    ok
}

fn first_positional_param(params: &[Param]) -> Option<&Param> {
    params.iter().find(|p| {
        !p.keyword
            && !p.rest
            && !p.from_keyword
            && !p.name.as_str().is_empty()
            && p.name.as_str() != "self"
    })
}

fn is_integer_lit(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Lit {
            value: Literal::Int { .. }
        }
    )
}

fn is_pure_super_body(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Super { .. } => true,
        ExprNode::Seq { exprs } if exprs.len() == 1 => is_pure_super_body(&exprs[0]),
        _ => false,
    }
}

fn body_contains_super(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(expr: &Expr, found: &mut bool) {
        if matches!(&*expr.node, ExprNode::Super { .. }) {
            *found = true;
            return;
        }
        expr.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
    found
}

/// `super(records)` → `@elements = records`.
/// Bare `super` forwards `initialize`'s first positional (zsuper).
/// Explicit `super()` → `@elements = []`. Unwrappable forms are
/// rejected upstream by `initialize_super_is_wrappable`.
fn rewrite_array_super_to_elements(expr: &mut Expr, params: &[Param]) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_array_super_to_elements(c, params));
    let elements = Symbol::from("elements");
    let assign = |value: Expr, span: Span| {
        Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Ivar {
                    name: elements.clone(),
                },
                value,
            },
        )
    };
    let empty = |span: Span| {
        Expr::new(
            span,
            ExprNode::Array {
                elements: vec![],
                style: crate::expr::ArrayStyle::default(),
            },
        )
    };
    let replacement = match &*expr.node {
        ExprNode::Super { args: Some(args) } if args.len() == 1 => {
            Some(assign(args[0].clone(), expr.span))
        }
        ExprNode::Super { args: Some(args) } if args.is_empty() => {
            Some(assign(empty(expr.span), expr.span))
        }
        ExprNode::Super { args: None } => first_positional_param(params).map(|p| {
            assign(
                Expr::new(
                    expr.span,
                    ExprNode::Var {
                        id: VarId(0),
                        name: p.name.clone(),
                    },
                ),
                expr.span,
            )
        }),
        _ => None,
    };
    if let Some(next) = replacement {
        *expr = next;
    }
}

fn call(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn local(name: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(name),
        },
    )
}

fn block_of(param: &str, body: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from(param)],
            rest_param: None,
            block_param: None,
            body,
            block_style: crate::expr::BlockStyle::Brace,
        },
    )
}

fn splat_local(name: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Splat { value: local(name) },
    )
}

fn yield_x() -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Yield {
            args: vec![local("x")],
        },
    )
}

fn elements_send(method: &str, args: Vec<Expr>, with_block: bool) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Ivar {
                    name: Symbol::from("elements"),
                },
            )),
            method: Symbol::from(method),
            args,
            block: with_block.then(|| block_of("x", yield_x())),
            parenthesized: true,
        },
    )
}

fn if_block_given(then_branch: Expr, else_branch: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: call(None, "block_given?", vec![]),
            then_branch,
            else_branch,
        },
    )
}

fn elements_ivar() -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Ivar {
            name: Symbol::from("elements"),
        },
    )
}

fn synth_method(
    owner: &ClassId,
    name: &str,
    params: Vec<Param>,
    body: Expr,
    has_block: bool,
) -> MethodDef {
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from(name),
        receiver: MethodReceiver::Instance,
        visibility: crate::dialect::MethodVisibility::Public,
        params,
        unsupported_formals: None,
        has_anonymous_block: has_block,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn synth_array_wrapper_methods(owner: &ClassId) -> Vec<MethodDef> {
    let rest = || vec![Param::rest(Symbol::from("args"))];
    let each_body = if_block_given(
        Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    elements_send("each", vec![], true),
                    Expr::new(Span::synthetic(), ExprNode::SelfRef),
                ],
            },
        ),
        call(Some(elements_ivar()), "each", vec![]),
    );
    PROTOCOL
        .iter()
        .map(|(name, kind)| match kind {
            Forward::Identity => synth_method(owner, name, vec![], elements_ivar(), false),
            Forward::Zero => synth_method(
                owner,
                name,
                vec![],
                call(Some(elements_ivar()), name, vec![]),
                false,
            ),
            Forward::One(param) => synth_method(
                owner,
                name,
                vec![Param::positional(Symbol::from(*param))],
                call(Some(elements_ivar()), name, vec![local(param)]),
                false,
            ),
            Forward::Splat => synth_method(
                owner,
                name,
                rest(),
                elements_send(name, vec![splat_local("args")], false),
                false,
            ),
            Forward::Enumerable => synth_method(
                owner,
                name,
                rest(),
                if_block_given(
                    elements_send(name, vec![splat_local("args")], true),
                    elements_send(name, vec![splat_local("args")], false),
                ),
                true,
            ),
            Forward::Each => synth_method(owner, name, vec![], each_body.clone(), true),
        })
        .collect()
}

fn synth_empty_array_initialize(owner: &ClassId) -> MethodDef {
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        visibility: crate::dialect::MethodVisibility::Private,
        params: vec![],
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar {
                    name: Symbol::from("elements"),
                },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Array {
                        elements: vec![],
                        style: crate::expr::ArrayStyle::default(),
                    },
                ),
            },
        ),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}
