//! `super(only: attrs)` inside a model's `as_json` →
//! `_as_json_only(attrs)` (lobsters' User/Message#as_json). Rails'
//! `Base#as_json` is the attribute serializer; the corpus only ever
//! calls it through `super` with an `only:` subset, so the runtime
//! gets ONE monomorphic method (`Array[Symbol] -> Hash[String,
//! untyped]`, connection.rb's `class Base` reopen) instead of an
//! options-hash `as_json` — an untyped opts walk is exactly the
//! shape the typed runtime refuses. Without the rewrite the `super`
//! has no superclass method anywhere in the emitted tree: CRuby
//! raises NoMethodError at runtime, spinel rejects at the C boundary
//! (spinel#2857). Other `super` shapes in `as_json` (bare, foreign
//! kwargs) stay verbatim — honest residue.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_as_json_super_grounding(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                if method.name.as_str() == "as_json" {
                    rewrite(&mut method.body);
                }
            }
        }
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let only_arg = match &mut *expr.node {
        ExprNode::Super { args: Some(args) } if args.len() == 1 => {
            match &mut *args[0].node {
                ExprNode::Hash { entries, .. } if entries.len() == 1 => {
                    let (k, v) = &mut entries[0];
                    let is_only = matches!(
                        &*k.node,
                        ExprNode::Lit { value: Literal::Sym { value } }
                            if value.as_str() == "only"
                    );
                    if is_only { Some(v.clone()) } else { None }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(arg) = only_arg {
        *expr.node = ExprNode::Send {
            recv: None,
            method: Symbol::from("_as_json_only"),
            args: vec![arg],
            block: None,
            parenthesized: true,
        };
        let _ = span;
    }
}
