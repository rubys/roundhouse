// Not left on the receiver: `permit`, `permit!`, `to_unsafe_h` and `require` are ActionController::Parameters methods, and a request's params reach the emitted program as plain String-keyed hashes.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_params_permit_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn is_param_value(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == crate::analyze::PARAM_VALUE)
}

fn is_request_params(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { recv: None, method, args, .. } => method.as_str() == "params" && args.is_empty(),
        ExprNode::Ivar { name } => name.as_str() == "params",
        _ => false,
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // A top-level `params.permit(:a, :b)` CONSUMED IN PLACE, as a call's
    // argument — the Rails 8 authentication generator's
    // `@user.update(params.permit(:password, :password_confirmation))`
    // and `User.authenticate_by(params.permit(:email_address,
    // :password))`. One RETURNED from a params helper (`def note_params
    // = params.permit(:body)`) is the controller lowering's: it becomes
    // that controller's typed params class.
    if let ExprNode::Send { args, .. } = &mut *expr.node {
        for arg in args.iter_mut() {
            if let Some(permitted) = permitted_chain(arg) {
                *arg = permitted;
            }
        }
    }
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else { return };
    let nested = is_param_value(r.ty.as_ref());
    let replacement = match (method.as_str(), args.as_slice()) {
        ("permit!" | "to_unsafe_h", []) if nested || is_request_params(r) => Some(r.clone()),
        ("permit", keys) if nested && !keys.is_empty() => {
            let Some(names) = keys.iter().map(sym_name).collect::<Option<Vec<_>>>() else { return };
            Some(send(expr.span, Some(r.clone()), "slice", names.into_iter().map(|n| str_lit(expr.span, n)).collect()))
        }
        ("require", [key]) if nested => {
            let Some(name) = sym_name(key) else { return };
            let params = Expr::new(expr.span, ExprNode::Const { path: vec![Symbol::from("Params")] });
            Some(send(expr.span, Some(params), "require_key", vec![r.clone(), str_lit(expr.span, name)]))
        }
        _ => None,
    };
    if let Some(mut new) = replacement {
        new.ty = expr.ty.clone();
        *expr = new;
    }
}

/// `params.permit(:a, :b)` on the request's params →
/// `Params.permitted(Params.permitted({}, params, "a", :a), params, "b", :b)`.
/// A model write reads Symbol keys, so this builds a Symbol-keyed hash
/// of the scalars the request provided, where `slice` would have handed
/// it String keys it never reads.
fn permitted_chain(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "permit" || args.is_empty() || !is_request_params(r) || is_param_value(r.ty.as_ref()) {
        return None;
    }
    let names = args.iter().map(sym_name).collect::<Option<Vec<_>>>()?;
    let params_const = || Expr::new(e.span, ExprNode::Const { path: vec![Symbol::from("Params")] });
    let mut acc = Expr::new(e.span, ExprNode::Hash { entries: vec![], kwargs: false });
    acc.ty = Some(permitted_ty());
    for name in names {
        let sym = Expr::new(e.span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name.as_str()) } });
        acc = send(e.span, Some(params_const()), "permitted", vec![acc, r.clone(), str_lit(e.span, name), sym]);
        acc.ty = Some(permitted_ty());
    }
    Some(acc)
}

/// What `Params.permitted` answers: the Symbol-keyed scalars.
fn permitted_ty() -> Ty {
    Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }
}

fn sym_name(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

fn str_lit(span: crate::span::Span, value: String) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value } });
    e.ty = Some(Ty::Str);
    e
}

fn send(span: crate::span::Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
    )
}
