use crate::analyze::indifferent::{self, Shape};
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

pub fn apply_indifferent_access_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut lower);
    for view in &mut app.views {
        lower(&mut view.body);
    }
    super::for_each_test_body(app, &mut lower);
}

fn lower(body: &mut Expr) {
    rewrite(body);
    // Not retyped during the rewrite: a parent call finds its indifferent receiver by the type its child still carries.
    retype(body);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    if !expr.ty.as_ref().is_some_and(|t| !matches!(t, Ty::Var { .. })) {
        return;
    }
    if let Some(mut replacement) = rewrite_node(expr.span, expr.decisions, &mut expr.node) {
        replacement.ty = expr.ty.take();
        *expr = replacement;
    }
}

fn rewrite_node(span: Span, decisions: u64, node: &mut ExprNode) -> Option<Expr> {
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = node else { return None };
    if decisions & crate::expr::REVERSE_MERGE != 0
        && let [hash] = args.as_slice()
        && hash.ty.as_ref().is_some_and(|t| indifferent::value_of(t).is_some())
        && !recv.ty.as_ref().is_some_and(|t| indifferent::value_of(t).is_some())
    {
        let defaults = recv.clone();
        *recv = from_hash(span, defaults);
        return None;
    }
    if method.as_str() == "new" && is_class_const(recv) && block.is_none() {
        return match args.as_slice() {
            [] => Some(Expr::new(span, ExprNode::Hash { entries: vec![], kwargs: false })),
            [hash] => Some(from_hash(span, hash.clone())),
            _ => None,
        };
    }
    if method.as_str() == "with_indifferent_access" && args.is_empty() && matches!(recv.ty, Some(Ty::Hash { .. })) {
        return Some(support_call(span, "indifferent", vec![recv.clone()]));
    }
    let typed = recv.ty.as_ref().is_some_and(is_indifferent_receiver);
    let maybe = read_from_indifferent(recv) || recv.ty.as_ref().is_some_and(may_be_indifferent);
    if !typed && !(maybe && converts_untyped(method.as_str())) {
        return None;
    }
    match indifferent::shape(method.as_str(), args, block.is_some())? {
        Shape::FirstKey => convert_key(&mut args[0]),
        Shape::AllKeys => {
            args.iter_mut().for_each(convert_key);
            if method.as_str() == "without" {
                *method = Symbol::from("except");
            }
        }
        Shape::Write => {
            convert_key(&mut args[0]);
            convert_assigned(&mut args[1]);
        }
        Shape::Merge => {
            let other = args[0].clone();
            args[0] = from_hash(span, other);
        }
        Shape::ReverseMerge => {
            return Some(Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(from_hash(span, args[0].clone())),
                    method: Symbol::from("merge"),
                    args: vec![recv.clone()],
                    block: None,
                    parenthesized: true,
                },
            ));
        }
        Shape::Copy => *method = Symbol::from("dup"),
        Shape::ToHash => return Some(support_call(span, "indifferent_to_hash", vec![recv.clone()])),
        Shape::SymbolizeKeys => {
            let hash = support_call(span, "indifferent_to_hash", vec![recv.clone()]);
            return Some(support_call(span, "symbolize_keys", vec![hash]));
        }
        Shape::Plain => {}
    }
    None
}

// Not every untyped value: only one read out of an indifferent hash, where every nested Hash is indifferent too.
fn read_from_indifferent(expr: &Expr) -> bool {
    let untyped = match &expr.ty {
        Some(Ty::Untyped) => true,
        Some(ty @ Ty::Union { .. }) => leaves(ty).contains(&&Ty::Untyped),
        _ => false,
    };
    if !untyped {
        return false;
    }
    let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node else { return false };
    matches!(method.as_str(), "[]" | "fetch" | "dig" | "first" | "last")
        && (recv.ty.as_ref().is_some_and(|t| is_indifferent_receiver(t) || may_be_indifferent(t)) || read_from_indifferent(recv))
}

// Not `include?`, `member?` or `delete`: an Array answers them for a Symbol too, so converting the key would change the answer.
fn converts_untyped(method: &str) -> bool {
    matches!(method, "[]" | "fetch" | "key?" | "has_key?" | "dig" | "values_at" | "slice" | "except" | "[]=" | "store" | "merge" | "update" | "merge!")
}

fn leaves(ty: &Ty) -> Vec<&Ty> {
    match ty {
        Ty::Union { variants } => variants.iter().flat_map(leaves).collect(),
        other => vec![other],
    }
}

fn may_be_indifferent(ty: &Ty) -> bool {
    leaves(ty).iter().any(|v| indifferent::value_of(v).is_some())
}

fn is_indifferent_receiver(ty: &Ty) -> bool {
    let leaves = leaves(ty);
    leaves.iter().any(|v| indifferent::value_of(v).is_some())
        && leaves.iter().all(|v| indifferent::value_of(v).is_some() || **v == Ty::Nil)
}

fn is_class_const(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Const { path }
        if matches!(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice(),
            ["ActiveSupport", "HashWithIndifferentAccess"]
            | ["", "ActiveSupport", "HashWithIndifferentAccess"]
            | ["HashWithIndifferentAccess"]
            | ["", "HashWithIndifferentAccess"]))
}

// Not converted again when already indifferent: Rails keeps that hash as it is, values shared.
fn from_hash(span: Span, hash: Expr) -> Expr {
    if hash.ty.as_ref().is_some_and(|t| indifferent::value_of(t).is_some()) {
        let ty = hash.ty.clone();
        let mut dup = Expr::new(
            span,
            ExprNode::Send { recv: Some(hash), method: Symbol::from("dup"), args: vec![], block: None, parenthesized: false },
        );
        dup.ty = ty;
        return dup;
    }
    support_call(span, "indifferent", vec![hash])
}

fn convert_key(key: &mut Expr) {
    if let ExprNode::Splat { value } = &mut *key.node {
        let span = value.span;
        let keys = value.clone();
        *value = support_call(span, "indifferent_keys", vec![keys]);
        return;
    }
    if let ExprNode::Lit { value: Literal::Sym { value } } = &*key.node {
        let text = value.as_str().to_string();
        *key.node = ExprNode::Lit { value: Literal::Str { value: text } };
        key.ty = Some(Ty::Str);
        return;
    }
    let span = key.span;
    match key.ty.as_ref() {
        Some(t) if !may_be_symbol(t) => {}
        Some(Ty::Sym) => {
            let sym = key.clone();
            *key = Expr::new(
                span,
                ExprNode::Send { recv: Some(sym), method: Symbol::from("to_s"), args: vec![], block: None, parenthesized: false },
            );
            key.ty = Some(Ty::Str);
        }
        _ => {
            let raw = key.clone();
            *key = support_call(span, "indifferent_key", vec![raw]);
        }
    }
}

fn may_be_symbol(ty: &Ty) -> bool {
    match ty {
        Ty::Sym | Ty::Untyped | Ty::Var { .. } => true,
        Ty::Union { variants } => variants.iter().any(may_be_symbol),
        _ => false,
    }
}

fn convert_assigned(value: &mut Expr) {
    let Some(ty) = value.ty.as_ref() else { return };
    if indifferent::value_of(ty).is_some() || !may_hold_hash(ty) {
        return;
    }
    let span = value.span;
    let raw = value.clone();
    let mut in_place = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } });
    in_place.ty = Some(Ty::Bool);
    *value = support_call(span, "indifferent_value", vec![raw, in_place]);
}

fn may_hold_hash(ty: &Ty) -> bool {
    match ty {
        Ty::Hash { .. } | Ty::Array { .. } | Ty::Untyped | Ty::Var { .. } | Ty::Record { .. } | Ty::Tuple { .. } => true,
        Ty::Union { variants } => variants.iter().any(may_hold_hash),
        _ => false,
    }
}

fn support_call(span: Span, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn retype(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut retype);
    if let Some(ty) = expr.ty.as_ref().filter(|t| indifferent::mentions(t)) {
        expr.ty = Some(indifferent::plain(ty));
    }
}
