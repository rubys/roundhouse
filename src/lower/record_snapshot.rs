//! Rebuilding a record from its raw attributes — Rails'
//! `Model.instantiate(attributes)` — as campfire's `RecordCache` does
//! with snapshots it took through `attributes_before_type_cast`:
//!
//! ```ruby
//! ActiveSupport::JSON.decode(snapshot).map { |name, attributes| name.constantize.instantiate(attributes) }
//! ```
//!
//! `Model.instantiate(attrs)` needs nothing: the per-model `instantiate`
//! the model lowering synthesizes reads its row String-keyed, as Rails'
//! does. What a compiled target cannot run is the class computed from a
//! String — there is no class object to send `instantiate` to
//! (matz/spinel#4217). The set of models is closed, so
//! `name.constantize.instantiate(attrs)` becomes
//! `ActiveRecord::Base.instantiate_named(name, attrs)`, which
//! `project::apply_instantiate_named` writes as a `case` over the app's
//! model names, each arm that model's `instantiate`, and an unknown name
//! raising `NameError` as `constantize` would. Only when `attrs` is a
//! plain read (a local, an ivar, a literal): Ruby evaluates
//! `constantize` — and raises — before the argument, and the rewrite
//! evaluates the argument first.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub fn apply_record_snapshot_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    super::for_each_test_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

fn plain_read(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. })
}

fn send(span: crate::span::Span, recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn constant(span: crate::span::Span, path: &[&str]) -> Expr {
    Expr::new(span, ExprNode::Const { path: path.iter().map(|s| Symbol::from(*s)).collect() })
}

fn rewrite_node(expr: &mut Expr) {
    let span = expr.span;
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "instantiate" || args.len() != 1 {
        return;
    }
    let ExprNode::Send { recv: Some(name), method: inner, args: inner_args, block: None, .. } = &*recv.node else {
        return;
    };
    if inner.as_str() != "constantize" || !inner_args.is_empty() || !plain_read(&args[0]) {
        return;
    }
    let name = name.clone();
    let attrs = args[0].clone();
    let base = constant(span, &["ActiveRecord", "Base"]);
    let mut call = send(span, base, "instantiate_named", vec![name, attrs]);
    call.ty = Some(Ty::Class { id: ClassId(Symbol::from("ActiveRecord::Base")), args: vec![].into() });
    *expr = call;
}
