//! `render …, status: 400` → `status: :bad_request` (and `head 404`,
//! `redirect_to …, status: 301`).
//!
//! Rails takes a status as either an Integer or a Symbol. The runtime's
//! `ActionController::Base#resolve_status` takes the Symbol only — it is
//! monomorphic so every strict target keeps a typed parameter — and
//! raises "Invalid HTTP status" on anything else, so an Integer literal
//! turned a deliberate 400 into a 500. lobsters writes Integers
//! throughout (`render plain: "can't find comment", status: 400`), and
//! its `require_logged_in_user_or_400` guard is on the anonymous path of
//! every logged-in-only page.
//!
//! The literal is knowable at transpile time, so it is grounded to the
//! Symbol Rails' own table names for it. The table is the runtime's
//! `STATUS_CODES` (runtime/ruby/action_controller/base.rb), compiled in
//! with `include_str!`, so the two cannot drift. A code the table does not
//! carry is left as written and keeps raising, as a status no HTTP
//! registry has should.

use std::sync::OnceLock;

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// `(symbol, code)` rows of the runtime's `STATUS_CODES`, in table order.
pub fn status_table() -> &'static [(String, u16)] {
    static TABLE: OnceLock<Vec<(String, u16)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // `include_str!`, not `runtime_files`: this pass runs in the wasm
        // playground too, and `runtime_files` is host-only.
        let src = include_str!("../../runtime/ruby/action_controller/base.rb");
        let start = src.find("STATUS_CODES = {").expect("STATUS_CODES table in base.rb");
        let body = &src[start..];
        let end = body.find('}').expect("STATUS_CODES table closes");
        body[..end]
            .lines()
            .skip(1)
            .filter_map(|line| {
                let line = line.split('#').next().unwrap_or("");
                let (name, code) = line.trim().trim_end_matches(',').split_once(':')?;
                Some((name.trim().to_string(), code.trim().parse().ok()?))
            })
            .collect()
    })
}

/// The symbol Rails names `code` by — the table's first row for it
/// (`:unprocessable_content` and its older alias share 422).
pub fn code_to_status_sym(code: u16) -> Option<&'static str> {
    status_table().iter().find(|(_, c)| *c == code).map(|(n, _)| n.as_str())
}

/// The code for a status symbol, when the table carries it.
pub fn status_sym_code(sym: &str) -> Option<u16> {
    status_table().iter().find(|(n, _)| n == sym).map(|(_, c)| *c)
}

pub fn apply_status_literal_lowering(app: &mut App) {
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    rewrite(&mut action.body);
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => rewrite(expr),
                _ => {}
            }
        }
    }
    for class in &mut app.library_classes {
        for m in &mut class.methods {
            rewrite(&mut m.body);
        }
    }
}

fn int_to_sym(e: &mut Expr) {
    let ExprNode::Lit { value: Literal::Int { value } } = &*e.node else { return };
    let Ok(code) = u16::try_from(*value) else { return };
    if let Some(sym) = code_to_status_sym(code) {
        *e.node = ExprNode::Lit { value: Literal::Sym { value: Symbol::from(sym) } };
        e.ty = None;
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
    match method.as_str() {
        "render" | "redirect_to" | "head" => {}
        _ => return,
    }
    // `head 404` — the status is the positional argument.
    if method.as_str() == "head" {
        if let Some(first) = args.first_mut() {
            int_to_sym(first);
        }
    }
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (k, v) in entries.iter_mut() {
            if matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "status") {
                int_to_sym(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_the_runtimes() {
        assert_eq!(code_to_status_sym(400), Some("bad_request"));
        assert_eq!(code_to_status_sym(404), Some("not_found"));
        assert_eq!(code_to_status_sym(301), Some("moved_permanently"));
        assert_eq!(status_sym_code("forbidden"), Some(403));
        assert_eq!(code_to_status_sym(299), None);
    }
}
