//! Lower `app.seeds` (the typed `db/seeds.rb` Expr) into a `Seeds`
//! LibraryClass with a single class method `run()`. The body is the
//! seeds Expr verbatim — analyze has already attached types and
//! effects, so the walker emits `Article.create!(...)`, etc., the
//! same way it would in a controller body.
//!
//! Self-describing IR: the seeds body's typing was set during
//! analyze (DbWrite on `create!`, etc.). The lowerer just wraps it
//! in a MethodDef shell — no per-target string-shape decisions.
//!
//! ONE IR rewrite happens here: Rails' has-many `create` shorthand
//! (`article.comments.create!(...)`) gets de-magic'd into the
//! explicit `Comment.create!(article_id: article.id, ...)` form
//! that targets without CollectionProxy semantics can dispatch.
//! The Ruby source stays Rails-idiomatic; the lowerer translates.
//! Mirrors `rewrite_assoc_through_parent_typed` in the controller
//! lowerer (which handles `build`/`find` for assignment-shape
//! contexts) — seeds are statement-shape so this pass only handles
//! the `create` form on its own.

use crate::App;
use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::lower::typing::fn_sig;
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Seeds` module as a single LibraryFunction:
/// `Seeds.run() -> nil` carrying the typed seeds Expr from
/// `app.seeds` verbatim. Empty when the app has no seeds file.
pub fn lower_seeds_to_library_functions(app: &App) -> Vec<LibraryFunction> {
    let Some(body) = app.seeds.as_ref().cloned() else {
        return Vec::new();
    };
    let body = rewrite_assoc_create(&body);
    let module_path = vec![Symbol::from("Seeds")];
    vec![LibraryFunction {
        module_path,
        name: Symbol::from("run"),
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
    }]
}

/// `<parent>.<assoc>.create(<args>)` → `<AssocClass>.create(<parent_class>_id: <parent>.id, ...args)`.
///
/// Pattern shape (after typer):
///   Send {                                     ← outer (create/create!)
///     recv: Some(Send {                        ← inner (assoc reader)
///       recv: Some(parent_expr),               ← typed Ty::Class { id: <Parent> }
///       method: <assoc> (e.g. "comments"),
///       args: [],
///     }),
///     method: "create" | "create!",
///     args: [Hash{...}] | [],
///   }
///
/// Transforms to:
///   Send {
///     recv: Some(Const(<AssocClass>)),
///     method: "create" | "create!",
///     args: [Hash{ <parent_class>_id: parent_expr.id, ...original_entries }],
///   }
///
/// `<AssocClass>` derives from singularize+camelize on the assoc
/// method name. `<parent_class>_id` derives from the parent_expr's
/// type (a Class type the typer set), snake_cased — e.g.
/// `Article` → `article_id`. Falls through (returns the expr
/// unchanged) when any of the shape preconditions don't hold.
pub fn rewrite_assoc_create(expr: &Expr) -> Expr {
    crate::lower::controller_to_library::util::map_expr(expr, &|e| {
        let ExprNode::Send {
            recv: Some(outer_recv),
            method: outer_method,
            args: outer_args,
            block: None,
            ..
        } = &*e.node
        else {
            return None;
        };
        let outer_method_str = outer_method.as_str();
        // `create` / `create!` map to the same-named class methods.
        // `build` is Rails-specific (instantiate without saving) —
        // map to `new` so Ruby/CRuby callers get `<Class>.new(attrs)`
        // and the TS emitter renders that as `new <Class>(attrs)`.
        let target_method = match outer_method_str {
            "create" | "create!" => outer_method.clone(),
            "build" => Symbol::from("new"),
            _ => return None,
        };
        let ExprNode::Send {
            recv: Some(parent_expr),
            method: assoc_method,
            args: inner_args,
            block: None,
            ..
        } = &*outer_recv.node
        else {
            return None;
        };
        if !inner_args.is_empty() {
            return None;
        }
        // Resolve parent class from the typer-set type. Without the
        // class type we can't derive the FK name; fall through.
        let parent_class = match parent_expr.ty.as_ref() {
            Some(Ty::Class { id, .. }) => id.0.as_str().to_string(),
            _ => return None,
        };
        let assoc_class = crate::naming::singularize_camelize(assoc_method.as_str());
        let fk = format!("{}_id", crate::naming::snake_case(&parent_class));

        // Build the FK-id expr: `<parent_expr>.id`.
        let parent_id = Expr::new(
            parent_expr.span,
            ExprNode::Send {
                recv: Some(parent_expr.clone()),
                method: Symbol::from("id"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let fk_entry = (
            Expr::new(
                e.span,
                ExprNode::Lit { value: crate::expr::Literal::Sym { value: Symbol::from(fk) } },
            ),
            parent_id,
        );

        // Merge FK entry with original kwargs/hash. Real-blog's
        // `article.comments.create!(commenter:, body:)` parses the
        // trailing kwargs as a single Hash arg.
        let merged_hash = match outer_args.first().map(|a| (&*a.node, a.span)) {
            Some((ExprNode::Hash { entries, braced }, span)) => {
                let mut new_entries = vec![fk_entry];
                new_entries.extend(entries.iter().cloned());
                Expr::new(span, ExprNode::Hash { entries: new_entries, braced: *braced })
            }
            _ => Expr::new(
                e.span,
                ExprNode::Hash { entries: vec![fk_entry], braced: false },
            ),
        };

        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    e.span,
                    ExprNode::Const { path: vec![Symbol::from(assoc_class)] },
                )),
                method: target_method,
                args: vec![merged_hash],
                block: None,
                parenthesized: true,
            },
        ))
    })
}

// `Span` used in synthesized expressions above.
#[allow(unused_imports)]
use Span as _Span;
