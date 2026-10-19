//! Shared lowering: counted Relation terminals.
//!
//! Typed `first(n)` / `last(n)` rename to `first_n` / `last_n` — one
//! method cannot carry both a record and an Array on a strict target.
//! The spelling is [`crate::catalog::counted_relation_terminal`], the
//! same predicate `scope_chain::counted_terminal` renames with.
//!
//! `rel.count > n` → `rel.more_than?(n)` (`SELECT 1 LIMIT 1 OFFSET n`).
//! Not `offset(n).exists?`: offset mutates, and loaded exists? ignores
//! it. Only a non-negative integer literal or Const (`PAGE_SIZE`).
//! Arrays and block forms stay Enumerable.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_relation_counted_terminals(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| rewrite(body));
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |c| rewrite(c));
    rewrite_count_gt_when(e, recv_is_relation);
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *e.node {
        if recv_is_relation(r) {
            let Some(renamed) =
                crate::catalog::counted_relation_terminal(method.as_str(), args.len(), false)
            else {
                return;
            };
            *method = Symbol::from(renamed);
        }
    }
}

/// Typed `Relation`, or a zero-arg `skip_preloading!` hop over an
/// untyped seed (`Page.load`'s `relation.skip_preloading!.last(size)`).
/// That method exists only on Relation in the runtime, so the hop is
/// enough proof for the rename — without it, an untyped parameter keeps
/// `.last(n)` and MRI raises arity error. (`preloaded` is Relation-only
/// too, but always arity-2 in emit/runtime, so it is not a hop here.)
fn recv_is_relation(e: &Expr) -> bool {
    if matches!(e.ty, Some(Ty::Relation { .. })) {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv: Some(inner), method, args, block: None, .. }
            if args.is_empty() && method.as_str() == "skip_preloading!" =>
        {
            matches!(&*inner.node, ExprNode::Var { .. } | ExprNode::Ivar { .. })
                || recv_is_relation(inner)
        }
        _ => false,
    }
}

/// `rel.count > n` → `rel.more_than?(n)` when `rel_ok` proves the
/// count's receiver is a Relation. Used from this pass (typed) and
/// from `scope_chain` (syntactic `__rel` / chain).
pub(crate) fn rewrite_count_gt_when(e: &mut Expr, rel_ok: impl Fn(&Expr) -> bool) -> bool {
    if !is_count_gt_shape(e, &rel_ok) {
        return false;
    }
    let parenthesized = true;
    let ExprNode::Send { recv: Some(count_e), args, .. } = &mut *e.node else {
        return false;
    };
    let threshold = args.remove(0);
    let ExprNode::Send { recv, .. } = &mut *count_e.node else {
        return false;
    };
    let Some(rel) = recv.take() else {
        return false;
    };
    *e.node = ExprNode::Send {
        recv: Some(rel),
        method: Symbol::from("more_than?"),
        args: vec![threshold],
        block: None,
        parenthesized,
    };
    e.ty = Some(Ty::Bool);
    true
}

fn is_count_gt_shape(e: &Expr, rel_ok: &impl Fn(&Expr) -> bool) -> bool {
    let ExprNode::Send { recv: Some(count_e), method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if method.as_str() != ">" || args.len() != 1 || !is_count_threshold(&args[0]) {
        return false;
    }
    let ExprNode::Send {
        recv: Some(rel),
        method: count_m,
        args: count_args,
        block: None,
        ..
    } = &*count_e.node
    else {
        return false;
    };
    count_m.as_str() == "count" && count_args.is_empty() && rel_ok(rel)
}

fn is_count_threshold(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { value: Literal::Int { value } } => *value >= 0,
        ExprNode::Const { .. } => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite;
    use crate::analyze::counted_first_last;
    use crate::catalog::{self, counted_relation_terminal};
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{ClassId, Symbol, TyVar, VarId};
    use crate::lower::scope_chain::counted_terminal;
    use crate::span::Span;
    use crate::ty::Ty;

    fn span() -> Span {
        Span::synthetic()
    }

    fn with_ty(node: ExprNode, ty: Option<Ty>) -> Expr {
        let mut expr = Expr::new(span(), node);
        expr.ty = ty;
        expr
    }

    fn int_arg() -> Expr {
        with_ty(ExprNode::Lit { value: Literal::Int { value: 2 } }, Some(Ty::Int))
    }

    fn var(name: &str, ty: Option<Ty>) -> Expr {
        with_ty(ExprNode::Var { id: VarId(0), name: Symbol::from(name) }, ty)
    }

    fn relation() -> Expr {
        var("rel", Some(Ty::Relation { of: ClassId(Symbol::from("Message")) }))
    }

    fn array() -> Expr {
        var("words", Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) }))
    }

    fn send(recv: Expr, name: &str, args: Vec<Expr>, block: bool) -> Expr {
        Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(name),
                args,
                block: block.then(|| with_ty(ExprNode::Lit { value: Literal::Nil }, None)),
                parenthesized: true,
            },
        )
    }

    fn method_of(expr: &Expr) -> &str {
        match &*expr.node {
            ExprNode::Send { method, .. } => method.as_str(),
            _ => panic!("expected a send"),
        }
    }

    fn scope_rename(name: &str, args: &[Expr], block: bool) -> Option<String> {
        let block_expr = block.then(|| with_ty(ExprNode::Lit { value: Literal::Nil }, None));
        counted_terminal(&Symbol::from(name), args, block_expr.as_ref()).map(|s| s.as_str().to_string())
    }

    /// The analyzer, `counted_terminal`, and `rewrite` read one predicate.
    /// Re-inlining a different `first` / `last` rule at any of the three
    /// fails this. Enumerable receivers keep `first` / `last`.
    #[test]
    fn counted_first_last_sites_share_one_predicate() {
        for entry in catalog::AR_CATALOG {
            if catalog::is_relation_builder_name(entry.name) {
                assert!(
                    counted_relation_terminal(entry.name, 1, false).is_none(),
                    "`{}` is a builder and must not be a counted rename",
                    entry.name,
                );
            }
        }
        assert!(!catalog::is_relation_builder_name("first"));
        assert!(!catalog::is_relation_builder_name("last"));

        let int = int_arg();
        for (name, runtime) in [("first", "first_n"), ("last", "last_n")] {
            assert_eq!(counted_relation_terminal(name, 1, false), Some(runtime));
            assert_eq!(scope_rename(name, &[int.clone()], false).as_deref(), Some(runtime));
            assert!(counted_first_last(&Symbol::from(name), &[int.clone()]));

            let mut on_rel = send(relation(), name, vec![int.clone()], false);
            rewrite(&mut on_rel);
            assert_eq!(method_of(&on_rel), runtime);

            let mut on_array = send(array(), name, vec![int.clone()], false);
            rewrite(&mut on_array);
            assert_eq!(method_of(&on_array), name, "Array#{name}(n) stays Enumerable");

            assert_eq!(counted_relation_terminal(name, 1, true), None);
            assert_eq!(scope_rename(name, &[int.clone()], true), None);
            let mut blocked = send(relation(), name, vec![int.clone()], true);
            rewrite(&mut blocked);
            assert_eq!(method_of(&blocked), name, "a block is not the counted rename");
            assert!(
                counted_first_last(&Symbol::from(name), &[int.clone()]),
                "dispatch does not see the block; both renamers do",
            );

            assert_eq!(counted_relation_terminal(name, 0, false), None);
            assert_eq!(scope_rename(name, &[], false), None);
            assert!(!counted_first_last(&Symbol::from(name), &[]));
        }

        // The type gate stays in the analyzer. A constant such as
        // PAGE_SIZE is still renamed once the receiver is a relation.
        let mut text = int_arg();
        text.node = Box::new(ExprNode::Lit { value: Literal::Str { value: "n".into() } });
        text.ty = Some(Ty::Str);
        assert!(!counted_first_last(&Symbol::from("last"), &[text.clone()]));
        assert_eq!(scope_rename("last", &[text.clone()], false).as_deref(), Some("last_n"));
        let mut on_rel = send(relation(), "last", vec![text], false);
        rewrite(&mut on_rel);
        assert_eq!(method_of(&on_rel), "last_n");

        let mut open = int_arg();
        open.ty = None;
        let mut gradual = int_arg();
        gradual.ty = Some(Ty::Untyped);
        let mut variable = int_arg();
        variable.ty = Some(Ty::Var { var: TyVar(0) });
        for arg in [open, gradual, variable] {
            assert!(counted_first_last(&Symbol::from("first"), &[arg]));
        }

        for name in ["take", "sample"] {
            assert_eq!(counted_relation_terminal(name, 1, false), None);
            assert_eq!(scope_rename(name, &[int.clone()], false), None);
            assert!(counted_first_last(&Symbol::from(name), &[int.clone()]));
            let mut on_rel = send(relation(), name, vec![int.clone()], false);
            rewrite(&mut on_rel);
            assert_eq!(method_of(&on_rel), name, "{name}(n) is typed as an Array and not renamed");
        }

        let hop = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(var("relation", None)),
                method: Symbol::from("skip_preloading!"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        );
        let mut hopped = send(hop, "last", vec![int], false);
        rewrite(&mut hopped);
        assert_eq!(method_of(&hopped), "last_n");
    }
}
