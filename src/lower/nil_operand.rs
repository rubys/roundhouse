//! A binary operator whose receiver may be nil, on a strict target.
//!
//! Ruby's `pages[0] < 1` compares when `pages[0]` holds an Integer and
//! raises NoMethodError when it is nil. The ruby family runs it as written.
//! A strict target has no operator on an optional, so the receiver is bound
//! once, checked, and read as its present type:
//!
//! ```ruby
//! (t = pages[0]; t.nil? ? raise(NoMethodError, "undefined method '<' for nil") : t < 1)
//! ```
//!
//! as an `ExprNode::Let`, so a receiver with a side effect runs once.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{Symbol, VarId};
use crate::ty::Ty;

/// Whether `lhs op rhs` fails only when `lhs` is nil: every present arm of
/// the nullable receiver takes the operator with this right operand.
pub(crate) fn nil_is_the_only_miss(op: &str, lhs: &Ty, rhs: &Ty) -> bool {
    let Ty::Union { variants } = lhs else { return false };
    let numeric = |t: &Ty| matches!(t, Ty::Int | Ty::Float);
    let present: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
    variants.iter().any(|v| matches!(v, Ty::Nil))
        && !present.is_empty()
        && present.iter().all(|v| match v {
            Ty::Int | Ty::Float => numeric(rhs) && matches!(op, "+" | "-" | "*" | "/" | "%" | "**" | "<" | ">" | "<=" | ">="),
            Ty::Str => match op {
                "+" | "<" | ">" | "<=" | ">=" => matches!(rhs, Ty::Str),
                "*" => matches!(rhs, Ty::Int),
                _ => false,
            },
            _ => false,
        })
}

/// The receiver's present type: the union without its nil arm.
fn present_ty(ty: &Ty) -> Ty {
    match ty {
        Ty::Union { variants } => {
            let present: Vec<Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).cloned().collect();
            match present.as_slice() {
                [one] => one.clone(),
                _ => Ty::Union { variants: present.into() },
            }
        }
        other => other.clone(),
    }
}

/// The app with its nil-receiver operators checked, for a strict target;
/// `None` for the ruby family, which runs them as written.
pub fn lower_for_target(app: &crate::App, target: crate::build_target::BuildTarget) -> Option<crate::App> {
    use crate::build_target::BuildTarget;
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
        return None;
    }
    let mut app = app.clone();
    lower_nil_operands(&mut app);
    Some(app)
}

pub fn lower_nil_operands(app: &mut crate::App) {
    let mut next = 0usize;
    crate::lower::for_each_emit_body(app, &mut |body| rewrite(body, &mut next));
}

fn rewrite(e: &mut Expr, next: &mut usize) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, next));
    let ExprNode::Send { recv: Some(lhs), method, args, block: None, .. } = &*e.node else { return };
    let [rhs] = args.as_slice() else { return };
    let (Some(lhs_ty), Some(rhs_ty)) = (lhs.ty.as_ref(), rhs.ty.as_ref()) else { return };
    if !nil_is_the_only_miss(method.as_str(), lhs_ty, rhs_ty) {
        return;
    }
    let span = e.span;
    let op = method.clone();
    let lhs = lhs.clone();
    let rhs = rhs.clone();
    let name = Symbol::from(format!("nil_operand_{}", *next));
    *next += 1;
    let read = |ty: Ty| Expr { ty: Some(ty), ..Expr::new(span, ExprNode::Var { id: VarId(0), name: name.clone() }) };
    let typed = |node: ExprNode, ty: Ty| Expr { ty: Some(ty), ..Expr::new(span, node) };
    let is_nil = typed(
        ExprNode::Send { recv: Some(read(lhs_ty.clone())), method: Symbol::from("nil?"), args: vec![], block: None, parenthesized: false },
        Ty::Bool,
    );
    let message = typed(ExprNode::Lit { value: Literal::Str { value: format!("undefined method '{}' for nil", op.as_str()) } }, Ty::Str);
    let error_class = typed(ExprNode::Const { path: vec![Symbol::from("NoMethodError")] }, Ty::Class { id: crate::ident::ClassId(Symbol::from("NoMethodError")), args: vec![].into() });
    let raise = typed(
        ExprNode::Send { recv: None, method: Symbol::from("raise"), args: vec![error_class, message], block: None, parenthesized: true },
        Ty::Nil,
    );
    let result_ty = e.ty.clone().unwrap_or(Ty::Untyped);
    let operate = typed(
        ExprNode::Send { recv: Some(read(present_ty(lhs_ty))), method: op, args: vec![rhs], block: None, parenthesized: false },
        result_ty.clone(),
    );
    let check = typed(ExprNode::If { cond: is_nil, then_branch: raise, else_branch: operate }, result_ty.clone());
    *e = typed(ExprNode::Let { id: VarId(0), name, value: lhs, body: check }, result_ty);
}
