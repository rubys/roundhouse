//! Conservative candidate analysis for methods whose optional block is
//! forwarded to a terminal guarded `capture` call.
//!
//! This is intentionally narrower than Ruby's Proc semantics. It recognizes
//! only the lowered nil-guarded capture shape and same-owner forwarding chains;
//! unresolved or ambiguous calls remain outside the contract.

use std::collections::{HashMap, HashSet};

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, Literal};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MethodKey {
    owner: String,
    receiver: MethodReceiver,
    name: String,
}

impl MethodKey {
    fn new(owner: &str, receiver: MethodReceiver, name: &str) -> Self {
        Self {
            owner: owner.to_string(),
            receiver,
            name: name.to_string(),
        }
    }
}

struct MethodRef<'a> {
    key: MethodKey,
    block_param: Option<&'a str>,
    body: &'a Expr,
}

/// Return methods whose optional block is proven to flow unchanged through a
/// same-owner call chain to the lowered nil-guarded capture terminal.
///
/// The proof is deliberately conservative: the terminal must be the method's
/// only use of its block parameter, and each forwarding method must pass that
/// parameter unchanged in exactly one same-receiver call. Cross-owner calls,
/// aliases, extra reads, recursion without a terminal, and ambiguous methods
/// do not become candidates. This does **not** prove the callers' block return
/// types; callsite evidence is required before using a String-returning Rust
/// ABI.
pub(crate) fn capture_forwarder_candidates<'a>(
    classes: impl IntoIterator<Item = &'a LibraryClass>,
    helper_owners: &HashMap<String, String>,
) -> HashSet<MethodKey> {
    let methods: Vec<MethodRef<'_>> = classes
        .into_iter()
        .flat_map(|class| {
            class.methods.iter().map(|method| MethodRef {
                key: MethodKey::new(class.name.0.as_str(), method.receiver, method.name.as_str()),
                block_param: method.block_param.as_ref().map(|param| param.name.as_str()),
                body: &method.body,
            })
        })
        .collect();

    let mut key_counts: HashMap<MethodKey, usize> = HashMap::new();
    for method in &methods {
        *key_counts.entry(method.key.clone()).or_default() += 1;
    }

    let mut proven: HashSet<MethodKey> = methods
        .iter()
        .filter_map(|method| {
            if key_counts.get(&method.key) != Some(&1) {
                return None;
            }
            let block_param = method.block_param?;
            (count_var_uses(method.body, block_param) == 2
                && !has_unsupported_block_flow(method.body, block_param)
                && has_one_guarded_capture(
                    method.body,
                    block_param,
                    &method.key,
                    &methods,
                    helper_owners,
                ))
            .then(|| method.key.clone())
        })
        .collect();

    loop {
        let mut newly_proven = Vec::new();
        for method in &methods {
            if proven.contains(&method.key) || key_counts.get(&method.key) != Some(&1) {
                continue;
            }
            let Some(block_param) = method.block_param else {
                continue;
            };
            if count_var_uses(method.body, block_param) != 1 {
                continue;
            }
            let forwards: Vec<MethodKey> = collect_forwarded_targets(method.body, block_param)
                .into_iter()
                .filter_map(|name| {
                    let matching: Vec<&MethodRef<'_>> = methods
                        .iter()
                        .filter(|candidate| {
                            candidate.key.owner == method.key.owner
                                && candidate.key.receiver == method.key.receiver
                                && candidate.key.name == name
                        })
                        .collect();
                    let [target] = matching.as_slice() else {
                        return None;
                    };
                    Some(target.key.clone())
                })
                .collect();
            if let [target] = forwards.as_slice()
                && proven.contains(target)
            {
                newly_proven.push(method.key.clone());
            }
        }
        if newly_proven.is_empty() {
            break;
        }
        proven.extend(newly_proven);
    }

    prove_string_callsites(&methods, proven, helper_owners)
}

#[derive(Clone, Debug)]
enum CallsiteBlock {
    None,
    StringLambda,
    Forwarded(String),
    Invalid,
}

#[derive(Clone, Debug)]
struct CandidateCallsite {
    caller: MethodKey,
    target: MethodKey,
    block: CallsiteBlock,
}

fn prove_string_callsites(
    methods: &[MethodRef<'_>],
    mut candidates: HashSet<MethodKey>,
    helper_owners: &HashMap<String, String>,
) -> HashSet<MethodKey> {
    let candidate_names: HashSet<&str> = candidates
        .iter()
        .map(|candidate| candidate.name.as_str())
        .collect();
    let mut callsites = Vec::new();
    let mut unresolved_names = HashSet::new();

    for caller in methods {
        visit_send_calls(
            caller.body,
            false,
            &mut |recv, name, block, inside_lambda| {
                let target = resolve_target(&caller.key, recv, name, helper_owners);
                let Some(target) = target else {
                    if candidate_names.contains(name) {
                        unresolved_names.insert(name.to_string());
                    }
                    return;
                };
                if !candidates.contains(&target) {
                    if candidate_names.contains(name) {
                        unresolved_names.insert(name.to_string());
                    }
                    return;
                }
                let block = match block {
                    None => CallsiteBlock::None,
                    Some(block)
                        if matches!(
                            &*block.node,
                            ExprNode::Lit {
                                value: Literal::Nil
                            }
                        ) =>
                    {
                        CallsiteBlock::None
                    }
                    Some(block) if is_zero_arg_string_lambda(block) => CallsiteBlock::StringLambda,
                    Some(Expr { node, .. })
                        if matches!(&**node, ExprNode::Var { name, .. }
                    if caller.block_param == Some(name.as_str()))
                            && !inside_lambda =>
                    {
                        CallsiteBlock::Forwarded(
                            caller
                                .block_param
                                .expect("matched block parameter")
                                .to_string(),
                        )
                    }
                    Some(_) => CallsiteBlock::Invalid,
                };
                callsites.push(CandidateCallsite {
                    caller: caller.key.clone(),
                    target,
                    block,
                });
            },
        );
    }

    loop {
        let before = candidates.len();
        let eligible_callers = candidates.clone();
        candidates.retain(|candidate| {
            !unresolved_names.contains(candidate.name.as_str())
                && callsites.iter().any(|site| &site.target == candidate)
                && callsites
                    .iter()
                    .filter(|site| &site.target == candidate)
                    .all(|site| match &site.block {
                        CallsiteBlock::None | CallsiteBlock::StringLambda => true,
                        CallsiteBlock::Forwarded(name) => {
                            eligible_callers.contains(&site.caller)
                                && methods.iter().any(|method| {
                                    method.key == site.caller
                                        && method.block_param == Some(name.as_str())
                                })
                        }
                        CallsiteBlock::Invalid => false,
                    })
        });

        let mut string_proven: HashSet<MethodKey> = callsites
            .iter()
            .filter(|site| matches!(site.block, CallsiteBlock::StringLambda))
            .map(|site| site.target.clone())
            .filter(|target| candidates.contains(target))
            .collect();
        loop {
            let mut newly_proven = Vec::new();
            for site in &callsites {
                if candidates.contains(&site.caller)
                    && candidates.contains(&site.target)
                    && matches!(site.block, CallsiteBlock::Forwarded(_))
                    && string_proven.contains(&site.caller)
                    && !string_proven.contains(&site.target)
                {
                    newly_proven.push(site.target.clone());
                }
            }
            if newly_proven.is_empty() {
                break;
            }
            string_proven.extend(newly_proven);
        }
        candidates.retain(|candidate| string_proven.contains(candidate));
        if candidates.len() == before {
            break;
        }
    }

    candidates
}

fn resolve_target(
    caller: &MethodKey,
    recv: Option<&Expr>,
    name: &str,
    helper_owners: &HashMap<String, String>,
) -> Option<MethodKey> {
    let (owner, receiver) = match recv.map(|recv| &*recv.node) {
        None => match helper_owners.get(name) {
            Some(owner) => (owner.clone(), MethodReceiver::Class),
            None => (caller.owner.clone(), caller.receiver),
        },
        Some(ExprNode::SelfRef) => (caller.owner.clone(), caller.receiver),
        Some(ExprNode::Const { path }) => (
            path.iter()
                .map(|segment| segment.as_str())
                .collect::<Vec<_>>()
                .join("::"),
            MethodReceiver::Class,
        ),
        Some(ExprNode::Var { .. }) => {
            let recv = recv?;
            match recv.ty.as_ref()? {
                crate::ty::Ty::Class { id, .. } => {
                    (id.0.as_str().to_string(), MethodReceiver::Instance)
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(MethodKey::new(&owner, receiver, name))
}

fn visit_send_calls(
    expr: &Expr,
    inside_lambda: bool,
    visit: &mut impl FnMut(Option<&Expr>, &str, Option<&Expr>, bool),
) {
    match &*expr.node {
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            ..
        } => {
            visit(
                recv.as_ref(),
                method.as_str(),
                block.as_ref(),
                inside_lambda,
            );
            if let Some(recv) = recv {
                visit_send_calls(recv, inside_lambda, visit);
            }
            for arg in args {
                visit_send_calls(arg, inside_lambda, visit);
            }
            if let Some(block) = block {
                visit_send_calls(block, inside_lambda, visit);
            }
        }
        ExprNode::Lambda { body, .. } => visit_send_calls(body, true, visit),
        _ => expr
            .node
            .for_each_child(&mut |child| visit_send_calls(child, inside_lambda, visit)),
    }
}

/// Reject block flows that a read-count and one-terminal check cannot prove
/// single-consumption and identity-preserving: loops can repeat the terminal,
/// `yield`/`super` can consume or forward the incoming block implicitly, and
/// assignment can replace the block without adding a Var read.
fn has_unsupported_block_flow(expr: &Expr, block_param: &str) -> bool {
    match &*expr.node {
        ExprNode::While { .. } | ExprNode::Yield { .. } | ExprNode::Super { .. } => true,
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
            matches!(target, crate::expr::LValue::Var { name, .. } if name.as_str() == block_param)
                || has_unsupported_block_flow_in_children(expr, block_param)
        }
        ExprNode::MultiAssign { targets, .. } => {
            targets.iter().any(|target| {
                matches!(target, crate::expr::LValue::Var { name, .. } if name.as_str() == block_param)
            }) || has_unsupported_block_flow_in_children(expr, block_param)
        }
        _ => has_unsupported_block_flow_in_children(expr, block_param),
    }
}

fn has_unsupported_block_flow_in_children(expr: &Expr, block_param: &str) -> bool {
    let mut found = false;
    expr.node
        .for_each_child(&mut |child| found |= has_unsupported_block_flow(child, block_param));
    found
}

fn has_one_guarded_capture(
    body: &Expr,
    block_param: &str,
    caller: &MethodKey,
    methods: &[MethodRef<'_>],
    helper_owners: &HashMap<String, String>,
) -> bool {
    let Some(owner) = helper_owners.get("capture") else {
        return false;
    };
    if owner != "ActionView::ViewHelpers" {
        return false;
    }
    let capture_target = MethodKey::new(owner, MethodReceiver::Class, "capture");
    let unique_target_exists = methods
        .iter()
        .filter(|method| method.key == capture_target)
        .count()
        == 1;
    unique_target_exists
        && resolve_target(caller, None, "capture", helper_owners).as_ref() == Some(&capture_target)
        && count_guarded_captures(body, block_param, caller, helper_owners, &capture_target) == 1
}

fn count_guarded_captures(
    expr: &Expr,
    block_param: &str,
    caller: &MethodKey,
    helper_owners: &HashMap<String, String>,
    capture_target: &MethodKey,
) -> usize {
    if is_guarded_capture(expr, block_param, caller, helper_owners, capture_target) {
        return 1;
    }
    if matches!(&*expr.node, ExprNode::Lambda { .. }) {
        return 0;
    }
    let mut count = 0;
    expr.node.for_each_child(&mut |child| {
        count += count_guarded_captures(child, block_param, caller, helper_owners, capture_target)
    });
    count
}

fn body_expression(body: &Expr) -> Option<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => match exprs.as_slice() {
            [expr] => Some(expr),
            _ => None,
        },
        _ => Some(body),
    }
}

fn is_guarded_capture(
    expr: &Expr,
    block_param: &str,
    caller: &MethodKey,
    helper_owners: &HashMap<String, String>,
    capture_target: &MethodKey,
) -> bool {
    let ExprNode::If {
        cond,
        then_branch,
        else_branch,
    } = &*expr.node
    else {
        return false;
    };
    let cond_is_nil_check = matches!(
        &*cond.node,
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } if method.as_str() == "nil?"
            && args.is_empty()
            && matches!(&*recv.node, ExprNode::Var { name, .. } if name.as_str() == block_param)
    );
    let then_is_empty_string = matches!(
        &*then_branch.node,
        ExprNode::Lit {
            value: Literal::Str { value }
        } if value.is_empty()
    );
    let else_is_capture = matches!(
        &*else_branch.node,
        ExprNode::Send {
            recv,
            method,
            args,
            block: Some(block),
            ..
        } if method.as_str() == "capture"
            && args.is_empty()
            && resolve_target(caller, recv.as_ref(), method.as_str(), helper_owners).as_ref()
                == Some(capture_target)
            && matches!(&*block.node, ExprNode::Var { name, .. } if name.as_str() == block_param)
    );
    cond_is_nil_check && then_is_empty_string && else_is_capture
}

fn collect_forwarded_targets(body: &Expr, block_param: &str) -> Vec<String> {
    let Some(expr) = body_expression(body) else {
        return Vec::new();
    };
    let ExprNode::Send {
        recv,
        method,
        block: Some(block),
        ..
    } = &*expr.node
    else {
        return Vec::new();
    };
    let is_local_receiver = recv
        .as_ref()
        .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef));
    if is_local_receiver
        && matches!(&*block.node, ExprNode::Var { name, .. } if name.as_str() == block_param)
    {
        vec![method.as_str().to_string()]
    } else {
        Vec::new()
    }
}

fn count_var_uses(body: &Expr, name: &str) -> usize {
    let mut count = 0;
    walk(body, &mut |expr| {
        if matches!(&*expr.node, ExprNode::Var { name: var, .. } if var.as_str() == name) {
            count += 1;
        }
    });
    count
}

fn walk(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    expr.node.for_each_child(&mut |child| walk(child, visit));
}

/// A block is eligible for the narrow owned-String callback ABI only when
/// its own IR proves zero arity and an exact String result. The analyzer's
/// synthesized `Ty::Fn.params` is empty even for parameterized lambdas, so
/// arity must come from the Lambda node itself.
fn is_zero_arg_string_lambda(expr: &Expr) -> bool {
    let ExprNode::Lambda {
        params,
        rest_param,
        extra_params,
        block_param,
        body,
        ..
    } = &*expr.node
    else {
        return false;
    };
    params.is_empty()
        && rest_param.is_none()
        && extra_params.is_empty()
        && block_param.is_none()
        && is_string_body(body)
        && !has_nonlocal_exit(body)
}

fn is_string_body(body: &Expr) -> bool {
    if !matches!(body.ty.as_ref(), Some(crate::ty::Ty::Str)) {
        return false;
    }
    let mut has_builder_hint = false;
    walk(body, &mut |expr| {
        has_builder_hint |= matches!(
            expr.hint,
            Some(
                IrHint::StringBuilderInit
                    | IrHint::StringBuilderAppend
                    | IrHint::StringBuilderResult
            )
        );
    });
    if !has_builder_hint {
        return true;
    }

    let ExprNode::Seq { exprs } = &*body.node else {
        return false;
    };
    let (Some(first), Some(last)) = (exprs.first(), exprs.last()) else {
        return false;
    };
    let ExprNode::Assign {
        target: crate::expr::LValue::Var {
            name: initialized, ..
        },
        ..
    } = &*first.node
    else {
        return false;
    };
    let ExprNode::Var { name: returned, .. } = &*last.node else {
        return false;
    };
    first.hint == Some(IrHint::StringBuilderInit)
        && last.hint == Some(IrHint::StringBuilderResult)
        && initialized == returned
}

fn has_nonlocal_exit(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Return { .. } | ExprNode::Break { .. } | ExprNode::Next { .. } => true,
        ExprNode::Lambda { .. } => false,
        _ => {
            let mut found = false;
            expr.node
                .for_each_child(&mut |child| found |= has_nonlocal_exit(child));
            found
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    fn string_lit(value: &str) -> Expr {
        let mut expr = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str {
                    value: value.to_string(),
                },
            },
        );
        expr.ty = Some(crate::ty::Ty::Str);
        expr
    }

    fn int_lit(value: i64) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Int { value },
            },
        )
    }

    fn call_with_block(method: &str, recv: Option<Expr>, block: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args: Vec::new(),
                block: Some(block),
                parenthesized: false,
            },
        )
    }

    fn terminal(block: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(var(block)),
                        method: Symbol::from("nil?"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                ),
                then_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: String::new(),
                        },
                    },
                ),
                else_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("capture"),
                        args: Vec::new(),
                        block: Some(var(block)),
                        parenthesized: false,
                    },
                ),
            },
        )
    }

    fn interpolated_terminal(block: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::StringInterp {
                parts: vec![
                    InterpPart::Text {
                        value: "<turbo-frame>".to_string(),
                    },
                    InterpPart::Expr {
                        expr: terminal(block),
                    },
                    InterpPart::Text {
                        value: "</turbo-frame>".to_string(),
                    },
                ],
            },
        )
    }

    fn forward(target: &str, block: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(target),
                args: Vec::new(),
                block: Some(var(block)),
                parenthesized: false,
            },
        )
    }

    fn string_lambda(params: &[&str], body: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                extra_params: Vec::new(),
                params: params.iter().map(|name| Symbol::from(*name)).collect(),
                rest_param: None,
                block_param: None,
                body,
                block_style: crate::expr::BlockStyle::Brace,
            },
        )
    }

    fn accumulator_lambda_body(init_name: &str, result_name: &str) -> Expr {
        let mut init = Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: crate::expr::LValue::Var {
                    id: crate::ident::VarId(0),
                    name: Symbol::from(init_name),
                },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: String::new(),
                        },
                    },
                ),
            },
        );
        init.hint = Some(IrHint::StringBuilderInit);
        let mut result = var(result_name);
        result.hint = Some(IrHint::StringBuilderResult);
        let mut body = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![init, result],
            },
        );
        body.ty = Some(crate::ty::Ty::Str);
        body
    }

    fn class(name: &str, methods: &[MethodDefStub<'_>]) -> LibraryClass {
        LibraryClass {
            name: crate::ident::ClassId(Symbol::from(name)),
            is_module: false,
            parent: None,
            parent_span: Span::synthetic(),
            includes: Vec::new(),
            methods: methods
                .iter()
                .map(|method| crate::dialect::MethodDef {
                    name: Symbol::from(method.name),
                    receiver: MethodReceiver::Instance,
                    visibility: Default::default(),
                    params: Vec::new(),
                    unsupported_formals: None,
                    has_anonymous_block: false,
                    block_param: method
                        .block
                        .map(|name| crate::dialect::Param::positional(Symbol::from(name))),
                    name_span: Span::synthetic(),
                    body: method.body.clone(),
                    signature: None,
                    effects: Default::default(),
                    enclosing_class: Some(Symbol::from(name)),
                    kind: Default::default(),
                    is_async: false,
                    mutates_self: false,
                })
                .collect(),
            class_ivar_initializers: Vec::new(),
            nullable_columns: Default::default(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        }
    }

    fn classify_with_framework_capture(classes: &[LibraryClass]) -> HashSet<MethodKey> {
        let mut capture_class = class(
            "ActionView::ViewHelpers",
            &[MethodDefStub {
                name: "capture",
                block: None,
                body: string_lit("captured"),
            }],
        );
        capture_class.methods[0].receiver = MethodReceiver::Class;
        let mut classes = classes.to_vec();
        classes.push(capture_class);
        let owners =
            HashMap::from([("capture".to_string(), "ActionView::ViewHelpers".to_string())]);
        capture_forwarder_candidates(classes.iter(), &owners)
    }

    struct MethodDefStub<'a> {
        name: &'a str,
        block: Option<&'a str>,
        body: Expr,
    }

    #[test]
    fn classifies_only_unambiguous_same_owner_forwarding_chains() {
        let methods = [
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: interpolated_terminal("block"),
            },
            MethodDefStub {
                name: "middle",
                block: Some("block"),
                body: forward("source", "block"),
            },
            MethodDefStub {
                name: "outer",
                block: Some("block"),
                body: forward("middle", "block"),
            },
            MethodDefStub {
                name: "render_html",
                block: None,
                body: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("outer"),
                        args: Vec::new(),
                        block: Some(string_lambda(&[], string_lit("html"))),
                        parenthesized: false,
                    },
                ),
            },
            MethodDefStub {
                name: "opaque",
                block: Some("block"),
                body: forward("not_proven", "block"),
            },
        ];
        let classes = [class("Probe", &methods)];
        let proven = classify_with_framework_capture(&classes);
        assert_eq!(proven.len(), 3, "proven methods: {proven:?}");
        for name in ["source", "middle", "outer"] {
            assert!(
                proven.contains(&MethodKey::new("Probe", MethodReceiver::Instance, name)),
                "{name} should be proven: {proven:?}"
            );
        }
        assert!(!proven.contains(&MethodKey::new("Probe", MethodReceiver::Instance, "opaque")));
    }

    #[test]
    fn rejects_extra_block_uses_and_ambiguous_method_targets() {
        let duplicate_targets = [
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: terminal("block"),
            },
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: terminal("block"),
            },
            MethodDefStub {
                name: "ambiguous_forwarder",
                block: Some("block"),
                body: forward("source", "block"),
            },
            MethodDefStub {
                name: "extra_use",
                block: Some("block"),
                body: Expr::new(
                    Span::synthetic(),
                    ExprNode::Seq {
                        exprs: vec![forward("source", "block"), var("block")],
                    },
                ),
            },
            MethodDefStub {
                name: "deferred_forwarder",
                block: Some("block"),
                body: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lambda {
                        extra_params: Vec::new(),
                        params: Vec::new(),
                        rest_param: None,
                        block_param: None,
                        body: forward("source", "block"),
                        block_style: crate::expr::BlockStyle::Brace,
                    },
                ),
            },
        ];
        let classes = [class("Probe", &duplicate_targets)];
        let proven = classify_with_framework_capture(&classes);
        assert!(
            proven.is_empty(),
            "ambiguous identities must not be proven: {proven:?}"
        );
        assert!(!proven.contains(&MethodKey::new(
            "Probe",
            MethodReceiver::Instance,
            "ambiguous_forwarder"
        )));
        assert!(!proven.contains(&MethodKey::new(
            "Probe",
            MethodReceiver::Instance,
            "extra_use"
        )));
        assert!(!proven.contains(&MethodKey::new(
            "Probe",
            MethodReceiver::Instance,
            "deferred_forwarder"
        )));
    }

    #[test]
    fn string_callsite_proof_requires_zero_arity_exact_string_and_local_control_flow() {
        let literal = string_lit("html");
        assert!(is_zero_arg_string_lambda(&string_lambda(
            &[],
            literal.clone()
        )));

        assert!(!is_zero_arg_string_lambda(&string_lambda(
            &["value"],
            literal.clone()
        )));

        let mut optional_result = literal.clone();
        optional_result.ty = Some(crate::ty::Ty::Union {
            variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil],
        });
        assert!(!is_zero_arg_string_lambda(&string_lambda(
            &[],
            optional_result
        )));

        let early_exit = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Return {
                            value: Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit {
                                    value: Literal::Int { value: 1 },
                                },
                            ),
                        },
                    ),
                    literal,
                ],
            },
        );
        let mut early_exit = early_exit;
        early_exit.ty = Some(crate::ty::Ty::Str);
        assert!(!is_zero_arg_string_lambda(&string_lambda(&[], early_exit)));

        assert!(is_zero_arg_string_lambda(&string_lambda(
            &[],
            accumulator_lambda_body("buf", "buf")
        )));
        assert!(!is_zero_arg_string_lambda(&string_lambda(
            &[],
            accumulator_lambda_body("outer", "inner")
        )));
    }

    #[test]
    fn candidate_seed_rejects_repeated_implicit_and_rebound_block_flow() {
        let looped = Expr::new(
            Span::synthetic(),
            ExprNode::While {
                cond: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Bool { value: true },
                    },
                ),
                body: terminal("block"),
                until_form: false,
            },
        );
        let yielded = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    Expr::new(Span::synthetic(), ExprNode::Yield { args: Vec::new() }),
                    terminal("block"),
                ],
            },
        );
        let rebound = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Assign {
                            target: crate::expr::LValue::Var {
                                id: crate::ident::VarId(0),
                                name: Symbol::from("block"),
                            },
                            value: string_lambda(&[], string_lit("replacement")),
                        },
                    ),
                    terminal("block"),
                ],
            },
        );
        for (name, body) in [
            ("looped", looped),
            ("yielded", yielded),
            ("rebound", rebound),
        ] {
            let methods = [
                MethodDefStub {
                    name,
                    block: Some("block"),
                    body,
                },
                MethodDefStub {
                    name: "good_callsite",
                    block: None,
                    body: call_with_block(name, None, string_lambda(&[], string_lit("valid"))),
                },
            ];
            let classes = [class("Probe", &methods)];
            let proven = classify_with_framework_capture(&classes);
            assert!(
                !proven.contains(&MethodKey::new("Probe", MethodReceiver::Instance, name)),
                "unsafe block flow `{name}` was classified: {proven:?}"
            );
        }
    }

    #[test]
    fn candidate_callsite_proof_descends_into_deferred_blocks() {
        let invalid_call = call_with_block("source", None, string_lambda(&[], int_lit(42)));
        let deferred = Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                extra_params: Vec::new(),
                params: Vec::new(),
                rest_param: None,
                block_param: None,
                body: invalid_call,
                block_style: crate::expr::BlockStyle::Brace,
            },
        );
        let methods = [
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: terminal("block"),
            },
            MethodDefStub {
                name: "good_callsite",
                block: None,
                body: call_with_block("source", None, string_lambda(&[], string_lit("valid"))),
            },
            MethodDefStub {
                name: "deferred_bad_callsite",
                block: None,
                body: deferred,
            },
        ];
        let classes = [class("Probe", &methods)];
        let proven = classify_with_framework_capture(&classes);
        assert!(
            !proven.contains(&MethodKey::new("Probe", MethodReceiver::Instance, "source")),
            "deferred non-String callsite was not considered: {proven:?}"
        );
    }

    #[test]
    fn unresolved_inherited_candidate_named_call_invalidates_candidate() {
        let parent_methods = [
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: terminal("block"),
            },
            MethodDefStub {
                name: "good_callsite",
                block: None,
                body: call_with_block("source", None, string_lambda(&[], string_lit("valid"))),
            },
        ];
        let child_methods = [MethodDefStub {
            name: "bad_callsite",
            block: None,
            body: call_with_block(
                "source",
                Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                string_lambda(&[], int_lit(42)),
            ),
        }];
        let classes = [
            class("Parent", &parent_methods),
            class("Child", &child_methods),
        ];
        let proven = classify_with_framework_capture(&classes);
        assert!(
            !proven.contains(&MethodKey::new(
                "Parent",
                MethodReceiver::Instance,
                "source"
            )),
            "unresolved inherited call was ignored: {proven:?}"
        );
    }

    #[test]
    fn guarded_terminal_requires_the_unique_shared_view_helpers_capture() {
        let methods = [
            MethodDefStub {
                name: "source",
                block: Some("block"),
                body: terminal("block"),
            },
            MethodDefStub {
                name: "good_callsite",
                block: None,
                body: call_with_block("source", None, string_lambda(&[], string_lit("valid"))),
            },
        ];
        let mut local_capture = class(
            "Probe",
            &[MethodDefStub {
                name: "capture",
                block: None,
                body: string_lit("not the framework implementation"),
            }],
        );
        local_capture.methods[0].receiver = MethodReceiver::Class;
        let mut classes = vec![class("Probe", &methods), local_capture];
        let owners = HashMap::from([("capture".to_string(), "Probe".to_string())]);
        let proven = capture_forwarder_candidates(classes.iter(), &owners);
        assert!(
            !proven.contains(&MethodKey::new("Probe", MethodReceiver::Instance, "source")),
            "local capture override was accepted as the framework terminal: {proven:?}"
        );

        classes.pop();
        let no_owner = capture_forwarder_candidates(classes.iter(), &HashMap::new());
        assert!(
            !no_owner.contains(&MethodKey::new("Probe", MethodReceiver::Instance, "source")),
            "unresolved capture terminal was accepted: {no_owner:?}"
        );
    }
}
