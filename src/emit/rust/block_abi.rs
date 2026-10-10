//! Conservative candidate analysis for methods whose optional block is
//! forwarded to a terminal guarded `capture` call.
//!
//! This is intentionally narrower than Ruby's Proc semantics. It recognizes
//! only the lowered nil-guarded capture shape and same-owner forwarding chains;
//! unresolved or ambiguous calls remain outside the contract.

use std::collections::{HashMap, HashSet};

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};

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
                && has_one_guarded_capture(method.body, block_param))
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

    proven
}

fn has_one_guarded_capture(body: &Expr, block_param: &str) -> bool {
    count_guarded_captures(body, block_param) == 1
}

fn count_guarded_captures(expr: &Expr, block_param: &str) -> usize {
    if is_guarded_capture(expr, block_param) {
        return 1;
    }
    if matches!(&*expr.node, ExprNode::Lambda { .. }) {
        return 0;
    }
    let mut count = 0;
    expr.node
        .for_each_child(&mut |child| count += count_guarded_captures(child, block_param));
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

fn is_guarded_capture(expr: &Expr, block_param: &str) -> bool {
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
            recv: None,
            method,
            args,
            block: Some(block),
            ..
        } if method.as_str() == "capture"
            && args.is_empty()
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
                name: "opaque",
                block: Some("block"),
                body: forward("not_proven", "block"),
            },
        ];
        let classes = [class("Probe", &methods)];
        let proven = capture_forwarder_candidates(classes.iter());
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
        let proven = capture_forwarder_candidates(classes.iter());
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
}
