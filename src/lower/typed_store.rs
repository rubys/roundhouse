//! `typed_store :<col> do |s| … end` — parse the DSL (the typed_store
//! gem: named attributes YAML-serialized into one TEXT column) out of a
//! model body. Two consumers: the Ruby emit path synthesizes overlay-
//! backed reader/writer methods from it, and the view lowering derives
//! which reader names are nilable scalars (an attr with no default reads
//! nil when unset, so `present?`/`blank?` on it need the nil-safe form).

use crate::dialect::ModelBodyItem;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub(crate) struct TypedStoreAttr {
    pub(crate) name: Symbol,
    pub(crate) is_bool: bool,
    pub(crate) default: Option<Expr>,
}

impl TypedStoreAttr {
    /// Reads nil when unset: no default, or an explicit `default: nil`.
    /// Bool attrs are excluded by callers that care about scalar
    /// emptiness predicates — their read sites are truthiness tests.
    pub(crate) fn nilable(&self) -> bool {
        match &self.default {
            None => true,
            Some(d) => matches!(&*d.node, ExprNode::Lit { value: Literal::Nil }),
        }
    }
}

/// Parse every `typed_store :<col> do |s| … end` declaration in a
/// model body into (column, attributes) pairs. Attribute lines are
/// `s.<type> :name[, default: <lit>, …]` sends on the block param;
/// anything else inside the block is ignored.
pub(crate) fn typed_store_decls(
    body: &[ModelBodyItem],
) -> Vec<(Symbol, Vec<TypedStoreAttr>)> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } =
            &*expr.node
        else {
            continue;
        };
        if method.as_str() != "typed_store" {
            continue;
        }
        let Some(col) = args.iter().find_map(|a| match &*a.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        }) else {
            continue;
        };
        let ExprNode::Lambda { params, body: block_body, .. } = &*block.node else {
            continue;
        };
        let Some(block_var) = params.first() else { continue };
        let stmts: Vec<&Expr> = match &*block_body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![block_body],
        };
        let mut attrs = Vec::new();
        for stmt in stmts {
            let ExprNode::Send { recv: Some(r), method: ty_m, args: a_args, .. } =
                &*stmt.node
            else {
                continue;
            };
            let recv_is_block_var = matches!(
                &*r.node,
                ExprNode::Var { name, .. } if name == block_var
            );
            if !recv_is_block_var {
                continue;
            }
            let Some(name) = a_args.iter().find_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            }) else {
                continue;
            };
            let default = a_args.iter().find_map(|a| match &*a.node {
                ExprNode::Hash { entries, .. } => entries.iter().find_map(|(k, v)| {
                    let is_default_key = match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str() == "default"
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            value == "default"
                        }
                        _ => false,
                    };
                    if is_default_key { Some(v.clone()) } else { None }
                }),
                _ => None,
            });
            attrs.push(TypedStoreAttr {
                name,
                is_bool: ty_m.as_str() == "boolean",
                default,
            });
        }
        if !attrs.is_empty() {
            out.push((col, attrs));
        }
    }
    out
}
