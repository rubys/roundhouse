//! Rewrite `<target> = <Class>.new(kw1: v1, kw2: v2, …)` into the
//! explicit setter chain that target-shape ARs and structs uniformly
//! support.
//!
//! Why this exists: kwarg construction is the canonical Rails idiom
//! for ad-hoc model instantiation (`Article.new(title: "")` in tests,
//! `Comment.new(commenter:, body:)` in seed/fixture code, etc.) but
//! it forces every strict-typed target to thread Symbol-keyed Hash
//! literals through the constructor and dispatch each field by name.
//! That's compiler-hostile across the board:
//!
//!   - Spinel emits `sp_<Class>_new(0)` (NULL Hash) for this shape
//!     today; the constructor body's `sp_StrIntHash_get(NULL, …)`
//!     then dereferences null. See matz/spinel#530.
//!   - Rust / Crystal / Go's struct-init syntax doesn't have a clean
//!     analog to Symbol-keyed kwarg-Hash dispatch; every target needs
//!     its own bridge.
//!   - Even targets that handle kwargs natively (Ruby, TypeScript)
//!     pay a runtime Hash literal alloc + N gets + N setter dispatches
//!     when explicit setters would be N direct field writes.
//!
//! Mirrors `feedback_monomorphize_polymorphic_apis`: when an idiom is
//! compiler-hostile across N targets, rewrite at the IR rather than
//! teach N emitters to handle it.
//!
//! Shape: `article = Foo.new(a: 1, b: 2)` rewrites to a statement-
//! position `Seq` that targets the same lvalue for setters:
//!
//!   ```ruby
//!   article = Foo.new
//!   article.a = 1
//!   article.b = 2
//!   ```
//!
//! Reusing the assignment target as the setter receiver avoids a
//! synthesized temp variable and keeps everything in statement
//! position — important for TypeScript, where `Assign` in expression
//! position drops the target binding (so a temp-Seq in expression
//! position would emit `tmp = X` as bare `X` and lose the binding).
//!
//! Rewrite invariant — fires only when ALL hold:
//!   - the node is an `Assign { target: Var | Ivar, value }`,
//!   - `value` is `Send { recv: Some(Const(_)), method: "new", args: [Hash{kwargs:true, …}] }`,
//!   - the Hash has at least one entry,
//!   - every key is a `Sym` literal.
//!
//! `Model.new(typed_params_struct)` (positional Params arg) and
//! `Model.new(some_hash_var)` (variable Hash arg) both fall through
//! unchanged. So do non-`Assign` call sites — `Article.new(title:"")
//! .save`, function-arg position, etc. — those are rare in lowered
//! IR but if they appear we leave them alone rather than risk a
//! broken expression-position rewrite.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

pub fn rewrite_new_kwargs(expr: &Expr) -> Expr {
    let rewritten = rewrite_assigns(expr);
    flatten_seq_in_seq(&rewritten)
}

/// `Seq([A, Seq([B, C]), D])` → `Seq([A, B, C, D])`. Applied
/// recursively. Direct-child Seqs only — `If { then_branch: Seq(…) }`
/// keeps its branch Seq intact (else-Seqs stay distinct from the
/// enclosing if's parent block). The typer treats a Seq as a scope
/// boundary; without flattening, vars assigned inside a rewrite-
/// synthesized Seq don't propagate to subsequent statements in the
/// outer body, leaving downstream `Var` reads with unresolved type
/// variables (`Ty::Var(TyVar(0))`).
fn flatten_seq_in_seq(e: &Expr) -> Expr {
    crate::lower::controller_to_library::util::map_expr(e, &|node| {
        let ExprNode::Seq { exprs } = &*node.node else {
            return None;
        };
        let has_nested_seq = exprs
            .iter()
            .any(|s| matches!(&*s.node, ExprNode::Seq { .. }));
        if !has_nested_seq {
            return None;
        }
        let mut flat: Vec<Expr> = Vec::with_capacity(exprs.len());
        for sub in exprs {
            if let ExprNode::Seq { exprs: inner } = &*sub.node {
                flat.extend(inner.iter().cloned());
            } else {
                flat.push(sub.clone());
            }
        }
        Some(Expr::new(node.span, ExprNode::Seq { exprs: flat }))
    })
}

fn rewrite_assigns(expr: &Expr) -> Expr {
    crate::lower::controller_to_library::util::map_expr(expr, &|e| {
        // Outer must be Assign with a singleton lvalue target.
        let ExprNode::Assign { target, value } = &*e.node else {
            return None;
        };
        if !matches!(target, LValue::Var { .. } | LValue::Ivar { .. }) {
            return None;
        }

        // Inner must be `<Const>.new(<one-kwarg-hash>)`.
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*value.node
        else {
            return None;
        };
        if method.as_str() != "new" {
            return None;
        }
        let ExprNode::Const { .. } = &*recv.node else {
            return None;
        };
        if args.len() != 1 {
            return None;
        }
        let ExprNode::Hash { entries, kwargs: true } = &*args[0].node else {
            return None;
        };
        if entries.is_empty() {
            return None;
        }

        // All keys must be Sym literals — kwargs are by construction
        // but be explicit so a future relaxation of the parser doesn't
        // silently break this.
        let mut kw_pairs: Vec<(Symbol, Expr)> = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: kn } } = &*k.node else {
                return None;
            };
            kw_pairs.push((kn.clone(), v.clone()));
        }

        let span = e.span;

        // Rebuild the setter receiver from the target lvalue. Same
        // VarId / name preserves the assignment binding; the typer
        // will re-thread types on the post-rewrite Seq.
        let make_recv = |span| -> Expr {
            match target {
                LValue::Var { id, name } => Expr::new(
                    span,
                    ExprNode::Var { id: *id, name: name.clone() },
                ),
                LValue::Ivar { name } => Expr::new(
                    span,
                    ExprNode::Ivar { name: name.clone() },
                ),
                _ => unreachable!("guarded above"),
            }
        };

        let zero_arg_new = Expr::new(
            recv.span,
            ExprNode::Send {
                recv: Some(recv.clone()),
                method: Symbol::from("new"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        );

        let mut exprs: Vec<Expr> = Vec::with_capacity(kw_pairs.len() + 1);
        exprs.push(Expr::new(
            span,
            ExprNode::Assign {
                target: target.clone(),
                value: zero_arg_new,
            },
        ));
        for (k, v) in kw_pairs {
            let setter = Symbol::from(format!("{}=", k.as_str()));
            exprs.push(Expr::new(
                v.span,
                ExprNode::Send {
                    recv: Some(make_recv(v.span)),
                    method: setter,
                    args: vec![v],
                    block: None,
                    parenthesized: true,
                },
            ));
        }

        Some(Expr::new(span, ExprNode::Seq { exprs }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn sym_lit(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } },
        )
    }

    fn str_lit(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: s.to_string() } },
        )
    }

    fn const_recv(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from(name)] },
        )
    }

    fn new_with_kwargs(class: &str, pairs: &[(&str, Expr)]) -> Expr {
        let entries: Vec<(Expr, Expr)> = pairs
            .iter()
            .map(|(k, v)| (sym_lit(k), v.clone()))
            .collect();
        let hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: true },
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(const_recv(class)),
                method: Symbol::from("new"),
                args: vec![hash],
                block: None,
                parenthesized: true,
            },
        )
    }

    fn assign_var(name: &str, value: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
                value,
            },
        )
    }

    #[test]
    fn rewrites_var_assign_of_kwargs_new_into_setter_seq() {
        let before = assign_var(
            "article",
            new_with_kwargs("Article", &[("title", str_lit("hi")), ("body", str_lit("x"))]),
        );
        let after = rewrite_new_kwargs(&before);

        let ExprNode::Seq { exprs } = &*after.node else {
            panic!("expected Seq, got {:?}", after.node);
        };
        // 1 Assign + 2 setters = 3 stmts. No trailing temp read —
        // the original assignment captures the value via its target.
        assert_eq!(exprs.len(), 3);

        // First: article = Article.new (zero-arg).
        let ExprNode::Assign {
            target: LValue::Var { name: tname, .. },
            value,
        } = &*exprs[0].node
        else {
            panic!("expected Assign as first Seq elem");
        };
        assert_eq!(tname.as_str(), "article");
        let ExprNode::Send { recv: Some(r), method, args, .. } = &*value.node else {
            panic!("expected zero-arg Send for new");
        };
        assert!(matches!(&*r.node, ExprNode::Const { .. }));
        assert_eq!(method.as_str(), "new");
        assert!(args.is_empty());

        // Setters target `article` directly (no temp var).
        for (i, key) in ["title", "body"].iter().enumerate() {
            let ExprNode::Send { recv: Some(r), method, args, .. } = &*exprs[i + 1].node else {
                panic!("expected Send for setter #{i}");
            };
            let ExprNode::Var { name, .. } = &*r.node else {
                panic!("expected Var as setter recv, got {:?}", r.node);
            };
            assert_eq!(name.as_str(), "article");
            assert_eq!(method.as_str(), &format!("{key}="));
            assert_eq!(args.len(), 1);
        }
    }

    #[test]
    fn rewrites_ivar_assign_of_kwargs_new() {
        let before = Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from("article") },
                value: new_with_kwargs("Article", &[("title", str_lit("hi"))]),
            },
        );
        let after = rewrite_new_kwargs(&before);

        let ExprNode::Seq { exprs } = &*after.node else {
            panic!("expected Seq");
        };
        assert_eq!(exprs.len(), 2);

        // Setter recv is `@article` (Ivar), matching the assignment target.
        let ExprNode::Send { recv: Some(r), method, .. } = &*exprs[1].node else {
            panic!("expected Send for setter");
        };
        let ExprNode::Ivar { name } = &*r.node else {
            panic!("expected Ivar as setter recv");
        };
        assert_eq!(name.as_str(), "article");
        assert_eq!(method.as_str(), "title=");
    }

    #[test]
    fn leaves_positional_arg_unchanged() {
        // `Article.new(article_params)` — positional Var arg.
        let positional = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from("article_params") },
        );
        let new_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(const_recv("Article")),
                method: Symbol::from("new"),
                args: vec![positional],
                block: None,
                parenthesized: true,
            },
        );
        let before = assign_var("article", new_call);
        let after = rewrite_new_kwargs(&before);
        // Outer Assign unchanged.
        assert!(matches!(&*after.node, ExprNode::Assign { .. }));
    }

    #[test]
    fn leaves_non_kwarg_hash_unchanged() {
        // `Article.new("title" => "x")` — string-keyed Hash, kwargs: false.
        let entries: Vec<(Expr, Expr)> = vec![(str_lit("title"), str_lit("hi"))];
        let hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: false },
        );
        let new_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(const_recv("Article")),
                method: Symbol::from("new"),
                args: vec![hash],
                block: None,
                parenthesized: true,
            },
        );
        let before = assign_var("article", new_call);
        let after = rewrite_new_kwargs(&before);
        assert!(matches!(&*after.node, ExprNode::Assign { .. }));
    }

    #[test]
    fn leaves_bare_send_unchanged() {
        // `Article.new(title: "x")` NOT in an Assign — function-arg
        // position, method-chain receiver, etc. We don't rewrite
        // because the temp-Seq form would lose the binding when
        // emitted in expression position by some targets (TS, …).
        let bare = new_with_kwargs("Article", &[("title", str_lit("hi"))]);
        let after = rewrite_new_kwargs(&bare);
        assert!(matches!(&*after.node, ExprNode::Send { .. }));
    }
}
