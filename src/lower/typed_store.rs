//! `typed_store :<col> do |s| … end` — the typed_store gem: named
//! attributes YAML-serialized into one TEXT column. This is the DSL's
//! one home: the parser (`typed_store_decls`), and the shared model
//! lowering's method synthesis (`push_typed_store_methods`) — per
//! attribute a reader, a `<name>?` predicate for booleans, and a
//! writer, each routing through the `TypedStore` runtime module
//! (`TypedStore.read/write` — the YAML seam; CRuby/JRuby trees ship it
//! in the overlay, strict targets carry the calls as ONE named runtime
//! seam until a native impl lands, same posture as Duration). The view
//! lowering also derives which reader names are nilable scalars from
//! the parse (an attr with no default reads nil when unset, so
//! `present?`/`blank?` on it need the nil-safe form).
//!
//! Signatures mirror what the analyzer registers for dispatch
//! (`analyze::register_typed_store` / `typed_store_ty`) so the
//! emitted RBS and the type-checker agree. Note the gem generates a
//! `?` predicate for EVERY attribute and the analyzer registers it so;
//! synthesis emits predicates for booleans only (the corpus shape) —
//! widening is a deliberate later step.

use super::model_to_library::push_synth_instance_method;
use crate::analyze::{typed_store_is_array, typed_store_ty};
use crate::dialect::{AccessorKind, MethodDef, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

pub(crate) struct TypedStoreAttr {
    pub(crate) name: Symbol,
    pub(crate) is_bool: bool,
    pub(crate) default: Option<Expr>,
    /// The `s.<type>` method name (`string`, `boolean`, `any`, …).
    pub(crate) decl_ty: Symbol,
    /// `array: true` — stores a list of the declared scalar type.
    pub(crate) is_array: bool,
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
                decl_ty: ty_m.clone(),
                is_array: typed_store_is_array(a_args),
            });
        }
        if !attrs.is_empty() {
            out.push((col, attrs));
        }
    }
    out
}

/// Synthesize the per-attribute accessor methods for every
/// `typed_store` declaration on `model`, into the shared model
/// lowering (all targets — the shared home of what used to be the
/// ruby-family emit pass `apply_typed_store_lowering`):
///
///   def <name>           = TypedStore.read(@<col>, "<name>", <default|nil>)
///   def <name>?          = same read body (boolean attrs only)
///   def <name>=(value)   = @<col> = TypedStore.write(@<col>, "<name>", value)
///
/// A custom method in the model body must win (`push_user_methods`
/// runs after this and drops collisions — the pass-4 inverted-dedupe
/// dance), and so must a name an earlier synthesizer claimed (schema
/// column accessors). Signatures come from the declared attribute type
/// (`typed_store_ty`), matching the analyzer's dispatch registration.
pub(crate) fn push_typed_store_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let stores = typed_store_decls(&model.body);
    for (col, attrs) in &stores {
        for a in attrs {
            let elem_ty = typed_store_ty(a.decl_ty.as_str()).unwrap_or(Ty::Untyped);
            // `array: true` stores a list of the scalar type; `any`
            // stays the gradual escape even as an array (mirrors
            // `register_typed_store_decls`).
            let attr_ty = if a.is_array && !matches!(elem_ty, Ty::Untyped) {
                Ty::Array { elem: Box::new(elem_ty) }
            } else {
                elem_ty
            };
            push_synth_instance_method(
                methods,
                model,
                a.name.clone(),
                Vec::new(),
                read_body(col, a),
                Some(super::model_to_library::fn_sig(vec![], attr_ty.clone())),
                AccessorKind::Method,
                false,
            );
            // The typed_store gem generates a `?` predicate for EVERY
            // attribute (the analyzer registers them so). Booleans
            // read through directly; everything else gets the typed
            // nil-check — `present?` on an `any`-typed read has no
            // AOT dispatch, and the corpus value (users/show's
            // `keybase_signatures?`) is nil-or-populated-hash, where
            // the nil test IS presence. (Divergence for empty-string/
            // empty-collection values: none in the corpus.)
            let pred_body = if a.is_bool {
                read_body(col, a)
            } else {
                let read = read_body(col, a);
                let nil_check = sp_expr(ExprNode::Send {
                    recv: Some(read),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                });
                sp_expr(ExprNode::Send {
                    recv: Some(nil_check),
                    method: Symbol::from("!"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                })
            };
            push_synth_instance_method(
                methods,
                model,
                Symbol::from(format!("{}?", a.name.as_str())),
                Vec::new(),
                pred_body,
                Some(super::model_to_library::fn_sig(vec![], Ty::Bool)),
                AccessorKind::Method,
                false,
            );
            let value = Symbol::from("value");
            push_synth_instance_method(
                methods,
                model,
                Symbol::from(format!("{}=", a.name.as_str())),
                vec![Param::positional(value.clone())],
                write_body(col, a),
                Some(super::model_to_library::fn_sig(vec![(value, attr_ty)], Ty::Nil)),
                AccessorKind::Method,
                true,
            );
        }
    }
}

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(col: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: col.clone() })
}

/// `TypedStore.read(@<col>, "<name>", <default|nil>)`.
fn read_body(col: &Symbol, a: &TypedStoreAttr) -> Expr {
    let default = a
        .default
        .clone()
        .unwrap_or_else(|| sp_expr(ExprNode::Lit { value: Literal::Nil }));
    sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("TypedStore")] })),
        method: Symbol::from("read"),
        args: vec![
            ivar_read(col),
            sp_expr(ExprNode::Lit {
                value: Literal::Str { value: a.name.as_str().to_string() },
            }),
            default,
        ],
        block: None,
        parenthesized: true,
    })
}

/// `@<col> = TypedStore.write(@<col>, "<name>", value)`.
fn write_body(col: &Symbol, a: &TypedStoreAttr) -> Expr {
    let write = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("TypedStore")] })),
        method: Symbol::from("write"),
        args: vec![
            ivar_read(col),
            sp_expr(ExprNode::Lit {
                value: Literal::Str { value: a.name.as_str().to_string() },
            }),
            sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("value") }),
        ],
        block: None,
        parenthesized: true,
    });
    sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: col.clone() },
        value: write,
    })
}
