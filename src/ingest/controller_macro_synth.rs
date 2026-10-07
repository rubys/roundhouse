//! Shared re-ingest envelope for class-body macros that expand to
//! private controller methods (+ optional `before_action` filters).
//!
//! `rate_limit` and `invisible_captcha` (and peers) synthesize Ruby,
//! re-ingest it under an isolated prism scope, then append the parsed
//! actions. Keep the parse / survey / PrivateMarker dance here once.

use crate::dialect::{Action, Controller, ControllerBodyItem, Filter, FilterKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// Re-ingest `method_bodies` (indented `def … end` source) as private
/// actions on `controller`. `tag` is the synthetic source label
/// (`<rate_limit>`, …) and appears in survey failure messages.
///
/// Returns `false` when synthesis failed or produced nothing (caller
/// should skip further work for this controller — and must not have
/// already committed filter replacements).
pub(super) fn append_private_actions(
    controller: &mut Controller,
    tag: &str,
    method_bodies: &str,
) -> bool {
    if method_bodies.is_empty() {
        return false;
    }
    let wrapped = format!("  private\n{method_bodies}");
    let Some(parsed_body) =
        reingest_controller_body(controller.name.0.as_str(), tag, &wrapped)
    else {
        return false;
    };
    let has_private_marker = controller
        .body
        .iter()
        .any(|item| matches!(item, ControllerBodyItem::PrivateMarker { .. }));
    if !has_private_marker {
        controller.body.push(ControllerBodyItem::PrivateMarker {
            leading_comments: Vec::new(),
            leading_blank_line: true,
        });
    }
    for item in parsed_body {
        if let ControllerBodyItem::Action { action, .. } = item {
            push_action(controller, action);
        }
    }
    true
}

/// Re-ingest arbitrary method source (not forced under `private`) and
/// return the parsed controller body items. Used by macros that mix
/// renames with synthesized public helpers (`impersonates`).
pub(super) fn reingest_controller_body(
    controller_name: &str,
    tag: &str,
    method_src: &str,
) -> Option<Vec<ControllerBodyItem>> {
    let class_src = format!("class {controller_name} < ApplicationController\n{method_src}\nend\n");
    let (result, diags) = crate::ingest::prism::scope(|| {
        super::controller::ingest_controller(class_src.as_bytes(), tag)
    });
    match (result, diags.is_empty()) {
        (Ok(Some(c)), true) => Some(c.body),
        (Ok(None), true) => None,
        (Ok(_), false) => {
            let label = tag.trim_matches(|c| c == '<' || c == '>');
            super::survey::record_synthesis_failure(
                tag,
                &format!("{label} forwarder for `{controller_name}`"),
                &diags,
            );
            None
        }
        (Err(err), _) => {
            super::survey::record(&err);
            None
        }
    }
}

pub(super) fn push_action(controller: &mut Controller, action: Action) {
    controller.body.push(ControllerBodyItem::Action {
        action,
        leading_comments: Vec::new(),
        leading_blank_line: true,
    });
}

/// Replace an `Unknown` class-body item with a synthetic `before_action`
/// targeting `method`. Shared by `rate_limit` / `invisible_captcha`.
pub(super) fn install_before_filter(
    item: &mut ControllerBodyItem,
    method: &str,
    only: Vec<Symbol>,
    except: Vec<Symbol>,
    prepend: bool,
    if_cond: Option<Symbol>,
    unless_cond: Option<Symbol>,
    if_cond_expr: Option<Expr>,
    unless_cond_expr: Option<Expr>,
) {
    let ControllerBodyItem::Unknown {
        leading_comments,
        leading_blank_line,
        ..
    } = item
    else {
        return;
    };
    let f = Filter {
        target_span: crate::span::Span::synthetic(),
        kind: FilterKind::Before,
        target: Symbol::from(method),
        from_concern: None,
        only,
        except,
        only_style: Default::default(),
        except_style: Default::default(),
        if_cond,
        unless_cond,
        if_cond_expr,
        unless_cond_expr,
        block: None,
        prepend,
    };
    *item = ControllerBodyItem::Filter {
        filter: f,
        leading_comments: std::mem::take(leading_comments),
        leading_blank_line: *leading_blank_line,
    };
}

/// `:create` / `[:create, :update]` → the names; anything else → None.
/// Shared by IR-level macro parsers (`rate_limit`, `invisible_captcha`, …).
pub(super) fn expr_symbol_list(v: &Expr) -> Option<Vec<Symbol>> {
    match &*v.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(vec![value.clone()]),
        ExprNode::Array { elements, .. } => elements
            .iter()
            .map(|e| match &*e.node {
                ExprNode::Lit {
                    value: Literal::Sym { value },
                } => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}
