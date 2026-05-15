//! Rewrite `<Class>.new(kw1: v1, kw2: v2, …)` into the explicit setter
//! chain that target-shape ARs and structs uniformly support.
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
//! The IR-level rewrite makes the explicit-setter shape canonical at
//! lowering time so every emit consumes the same primitive form.
//! Mirrors `feedback_monomorphize_polymorphic_apis`: when an idiom
//! is compiler-hostile across N targets, rewrite at the IR rather
//! than teach N emitters to handle it.
//!
//! Shape: `Foo.new(a: 1, b: 2)` rewrites to a `Seq` containing
//!
//!   ```ruby
//!   tmp = Foo.new
//!   tmp.a = 1
//!   tmp.b = 2
//!   tmp
//!   ```
//!
//! where `tmp` is a synthesized var named `__new_<Class>_tmp`. The
//! same name reuses across rewrites for the same class — harmless
//! since the var gets reassigned at each use.
//!
//! Rewrite invariant — fires only when:
//!   - receiver is a `Const` (class literal),
//!   - method is exactly `"new"`,
//!   - args is exactly one `Hash` with `kwargs: true`,
//!   - every key in that Hash is a `Sym` literal.
//!
//! `Model.new(typed_params_struct)` (positional Params arg) and
//! `Model.new(some_hash_var)` (variable Hash arg) both fall through
//! unchanged.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

pub fn rewrite_new_kwargs(expr: &Expr) -> Expr {
    crate::lower::controller_to_library::util::map_expr(expr, &|e| {
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*e.node
        else {
            return None;
        };
        if method.as_str() != "new" {
            return None;
        }
        let ExprNode::Const { path } = &*recv.node else {
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

        let class_name = path
            .last()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "X".to_string());
        let tmp_name = Symbol::from(format!("__new_{class_name}_tmp"));
        let span = e.span;

        let tmp_read = || Expr::new(span, ExprNode::Var { id: VarId(0), name: tmp_name.clone() });

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

        let mut exprs: Vec<Expr> = Vec::with_capacity(kw_pairs.len() + 2);
        exprs.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: tmp_name.clone() },
                value: zero_arg_new,
            },
        ));
        for (k, v) in kw_pairs {
            let setter = Symbol::from(format!("{}=", k.as_str()));
            exprs.push(Expr::new(
                v.span,
                ExprNode::Send {
                    recv: Some(tmp_read()),
                    method: setter,
                    args: vec![v],
                    block: None,
                    parenthesized: true,
                },
            ));
        }
        exprs.push(tmp_read());

        Some(Expr::new(span, ExprNode::Seq { exprs }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::Symbol;
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

    fn kwargs_send(class: &str, pairs: &[(&str, Expr)]) -> Expr {
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

    #[test]
    fn rewrites_kwargs_new_into_seq_with_setters() {
        let before = kwargs_send("Article", &[("title", str_lit("hi")), ("body", str_lit("x"))]);
        let after = rewrite_new_kwargs(&before);
        let ExprNode::Seq { exprs } = &*after.node else {
            panic!("expected Seq, got {:?}", after.node);
        };
        // 1 Assign + 2 setters + 1 trailing Var read = 4
        assert_eq!(exprs.len(), 4);

        // First: tmp = Article.new
        let ExprNode::Assign { target: LValue::Var { name: tmp_name, .. }, value } = &*exprs[0].node
        else {
            panic!("expected Assign as first Seq elem");
        };
        assert_eq!(tmp_name.as_str(), "__new_Article_tmp");
        let ExprNode::Send { recv: Some(r), method, args, .. } = &*value.node else {
            panic!("expected zero-arg Send for new");
        };
        assert!(matches!(&*r.node, ExprNode::Const { .. }));
        assert_eq!(method.as_str(), "new");
        assert!(args.is_empty());

        // Setter calls — each Send on tmp, method "<key>=", arg = value.
        for (i, key) in ["title", "body"].iter().enumerate() {
            let ExprNode::Send { recv: Some(r), method, args, .. } = &*exprs[i + 1].node else {
                panic!("expected Send for setter #{i}");
            };
            let ExprNode::Var { name, .. } = &*r.node else {
                panic!("expected tmp Var as setter recv");
            };
            assert_eq!(name.as_str(), "__new_Article_tmp");
            assert_eq!(method.as_str(), &format!("{key}="));
            assert_eq!(args.len(), 1);
        }

        // Trailing Var read.
        let ExprNode::Var { name, .. } = &*exprs[3].node else {
            panic!("expected Var as last Seq elem");
        };
        assert_eq!(name.as_str(), "__new_Article_tmp");
    }

    #[test]
    fn leaves_positional_arg_unchanged() {
        // `Article.new(article_params)` — single positional arg that
        // isn't a Hash literal. Must not rewrite.
        let positional = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from("article_params") },
        );
        let before = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(const_recv("Article")),
                method: Symbol::from("new"),
                args: vec![positional],
                block: None,
                parenthesized: true,
            },
        );
        let after = rewrite_new_kwargs(&before);
        // Outer node unchanged — same Send shape.
        assert!(matches!(&*after.node, ExprNode::Send { .. }));
    }

    #[test]
    fn leaves_non_kwarg_hash_unchanged() {
        // `Article.new("title" => "x")` — string-keyed Hash with
        // `kwargs: false`. Not a kwarg form; leave alone.
        let entries: Vec<(Expr, Expr)> = vec![(str_lit("title"), str_lit("hi"))];
        let hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: false },
        );
        let before = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(const_recv("Article")),
                method: Symbol::from("new"),
                args: vec![hash],
                block: None,
                parenthesized: true,
            },
        );
        let after = rewrite_new_kwargs(&before);
        assert!(matches!(&*after.node, ExprNode::Send { .. }));
    }

    #[test]
    fn leaves_non_const_recv_unchanged() {
        // `obj.new(...)` — receiver is a method call, not a Const.
        // Rare but possible; leave alone.
        let recv = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from("obj") },
        );
        let entries = vec![(sym_lit("title"), str_lit("hi"))];
        let hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: true },
        );
        let before = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("new"),
                args: vec![hash],
                block: None,
                parenthesized: true,
            },
        );
        let after = rewrite_new_kwargs(&before);
        assert!(matches!(&*after.node, ExprNode::Send { .. }));
    }
}
