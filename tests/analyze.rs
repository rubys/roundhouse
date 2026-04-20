//! Analyzer smoke test: types land on expressions we can verify.
//!
//! Keep these tests specific — pick a location in the IR that has an
//! unambiguous expected type, and assert it. Broader coverage goes in
//! snapshot tests once we have them.

use std::path::Path;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::effect::Effect;
use roundhouse::expr::{ExprNode, LValue, Literal};
use roundhouse::ingest::ingest_app;
use roundhouse::ty::Ty;
use roundhouse::{ClassId, Symbol, TableRef};

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

fn analyzed_app() -> roundhouse::App {
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn post_all_has_type_array_of_post() {
    let app = analyzed_app();
    let index = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "index")
        .unwrap();
    // body is `@posts = Post.all`. value's ty should be Array<Post>.
    let ExprNode::Assign { value, .. } = &*index.body.node else {
        panic!("expected Assign at top of index body");
    };
    match value.ty.as_ref().expect("analyzer populated value.ty") {
        Ty::Array { elem } => match &**elem {
            Ty::Class { id, args } => {
                assert_eq!(id, &ClassId(Symbol::from("Post")));
                assert!(args.is_empty());
            }
            other => panic!("expected Array<Post>, got Array<{other:?}>"),
        },
        other => panic!("expected Array, got {other:?}"),
    }
}

#[test]
fn post_find_has_type_post() {
    let app = analyzed_app();
    let show = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "show")
        .unwrap();
    let ExprNode::Assign { value, .. } = &*show.body.node else {
        panic!("expected Assign");
    };
    // value is `Post.find(params[:id])`; ty should be Post (Class).
    match value.ty.as_ref().expect("ty populated") {
        Ty::Class { id, .. } => assert_eq!(id, &ClassId(Symbol::from("Post"))),
        other => panic!("expected Class(Post), got {other:?}"),
    }
}

#[test]
fn literals_get_primitive_types() {
    let app = analyzed_app();
    let post_model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .expect("Post model");
    let scope = post_model.scopes().next().expect("scope 0");
    // Scope body: `limit(10)` — the 10 is an Int literal.
    let ExprNode::Send { args, .. } = &*scope.body.node else {
        panic!("scope body is {:?}", scope.body.node);
    };
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].ty, Some(Ty::Int));
}

#[test]
fn const_ref_has_class_type() {
    let app = analyzed_app();
    let index = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "index")
        .unwrap();
    // RHS is Send(Some(Const(Post)), all, []). Inner Const should have ty Class(Post).
    let ExprNode::Assign { value, .. } = &*index.body.node else { panic!() };
    let ExprNode::Send { recv, .. } = &*value.node else { panic!() };
    let recv = recv.as_ref().expect("explicit receiver");
    match recv.ty.as_ref().expect("ty populated") {
        Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Post"),
        other => panic!("expected Class(Post), got {other:?}"),
    }
}

#[test]
fn assign_target_ivar_is_still_structural() {
    // Sanity check: the analyzer doesn't corrupt non-expression structure.
    let app = analyzed_app();
    let index = app.controllers[0].actions().next().expect("first action");
    let ExprNode::Assign { target, .. } = &*index.body.node else { panic!() };
    match target {
        LValue::Ivar { name } => assert_eq!(name.as_str(), "posts"),
        other => panic!("expected Ivar, got {other:?}"),
    }
}

#[test]
fn params_resolves_via_implicit_self_in_action_body() {
    let app = analyzed_app();
    let show = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "show")
        .unwrap();
    // Body: `@post = Post.find(params[:id])`.
    // Drill into the arg of find: it's the `params[:id]` Send.
    let ExprNode::Assign { value, .. } = &*show.body.node else { panic!() };
    let ExprNode::Send { args, .. } = &*value.node else { panic!() };
    assert_eq!(args.len(), 1);
    let bracket_send = &args[0];
    // bracket_send is `params[:id]` — Send(Some(params), "[]", [:id])
    // Its receiver is the bare `params` call.
    let ExprNode::Send { recv, method, .. } = &*bracket_send.node else { panic!() };
    assert_eq!(method.as_str(), "[]");
    let params_recv = recv.as_ref().expect("bracket has a receiver");

    // `params` (implicit self Send) — now resolved via ctx.self_ty to Hash<Sym, Str>.
    match params_recv.ty.as_ref().expect("params ty populated") {
        Ty::Hash { key, value } => {
            assert!(matches!(**key, Ty::Sym));
            assert!(matches!(**value, Ty::Str));
        }
        other => panic!("expected Hash<Sym, Str>, got {other:?}"),
    }

    // `params[:id]` resolves to Union<Str, Nil>.
    match bracket_send.ty.as_ref().expect("bracket ty populated") {
        Ty::Union { variants } => {
            assert!(variants.iter().any(|v| matches!(v, Ty::Str)));
            assert!(variants.iter().any(|v| matches!(v, Ty::Nil)));
        }
        other => panic!("expected Union<Str, Nil>, got {other:?}"),
    }
}

#[test]
fn scope_body_self_is_model_class() {
    // `scope :recent, -> { limit(10) }` — `limit` is a bare call; self must
    // resolve to the model class so that `limit` dispatches to the class
    // method returning Array<Post>.
    let app = analyzed_app();
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    let scope = post.scopes().next().expect("first scope");
    // Scope body: `limit(10)` — the top-level Send's ty should be Array<Post>.
    match scope.body.ty.as_ref().expect("scope body ty populated") {
        Ty::Array { elem } => match &**elem {
            Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Post"),
            other => panic!("expected Array<Post>, got Array<{other:?}>"),
        },
        other => panic!("expected Array, got {other:?}"),
    }
}

#[test]
fn hash_literal_in_where_call_types_as_hash() {
    // `scope :published, -> { where(published: true) }`
    // The `published: true` kwarg is a Hash literal (braced: false in IR).
    let app = analyzed_app();
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    let published = post
        .scopes()
        .find(|s| s.name.as_str() == "published")
        .expect("published scope");
    // Body: `where(published: true)` — Send(None, where, [Hash{published: true}])
    let ExprNode::Send { args, .. } = &*published.body.node else {
        panic!("expected Send at scope body");
    };
    assert_eq!(args.len(), 1, "where takes one hash arg");
    match &*args[0].node {
        ExprNode::Hash { entries, braced } => {
            assert!(!*braced, "trailing kwargs form should be unbraced");
            assert_eq!(entries.len(), 1);
            // Key is Sym(published), value is true.
            match &*entries[0].0.node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    assert_eq!(value.as_str(), "published");
                }
                other => panic!("expected Sym key, got {other:?}"),
            }
            match &*entries[0].1.node {
                ExprNode::Lit { value: Literal::Bool { value: true } } => {}
                other => panic!("expected Bool(true), got {other:?}"),
            }
        }
        other => panic!("expected Hash, got {other:?}"),
    }
    // The Hash expression's ty should be Hash<Sym, Bool>.
    match args[0].ty.as_ref().expect("hash ty populated") {
        Ty::Hash { key, value } => {
            assert!(matches!(**key, Ty::Sym));
            assert!(matches!(**value, Ty::Bool));
        }
        other => panic!("expected Hash<Sym, Bool>, got {other:?}"),
    }
}

#[test]
fn if_branches_union_merge() {
    // create body ends with:
    //   if @post.save
    //     redirect_to @post
    //   else
    //     render :new
    //   end
    // Both branches are `redirect_to` / `render` which return Nil per the
    // ApplicationController synthetic methods. The If's type should be the
    // merged union — since both are Nil, the union collapses to Nil.
    let app = analyzed_app();
    let create = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "create")
        .expect("create action");
    let ExprNode::Seq { exprs } = &*create.body.node else {
        panic!("expected Seq body");
    };
    let last = exprs.last().unwrap();
    let ExprNode::If { .. } = &*last.node else {
        panic!("expected If as last stmt, got {:?}", last.node);
    };
    match last.ty.as_ref().expect("If ty populated") {
        Ty::Nil => {} // both branches Nil -> union_of collapses
        other => panic!("expected Nil, got {other:?}"),
    }
}

#[test]
fn ivar_read_resolves_through_seq_tracking() {
    // destroy body:
    //   @post = Post.find(params[:id])
    //   @post.destroy
    //   redirect_to posts_path
    //
    // The second statement's @post receiver must type as Post via the
    // ivar binding that the first statement established.
    let app = analyzed_app();
    let ctrl = &app.controllers[0];
    let destroy = ctrl
        .actions()
        .find(|a| a.name.as_str() == "destroy")
        .expect("destroy action");
    let ExprNode::Seq { exprs } = &*destroy.body.node else {
        panic!("expected Seq body, got {:?}", destroy.body.node);
    };
    assert!(exprs.len() >= 2, "need at least two stmts");

    // stmt[1] is `@post.destroy` — a Send whose receiver is an Ivar.
    let ExprNode::Send { recv, method, .. } = &*exprs[1].node else {
        panic!("expected Send at stmt[1]");
    };
    assert_eq!(method.as_str(), "destroy");
    let recv = recv.as_ref().expect("@post.destroy has a receiver");
    let ExprNode::Ivar { name } = &*recv.node else {
        panic!("expected Ivar receiver");
    };
    assert_eq!(name.as_str(), "post");

    match recv.ty.as_ref().expect("@post ty populated") {
        Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Post"),
        other => panic!("expected @post : Post, got {other:?}"),
    }
}

#[test]
fn action_effects_include_db_reads() {
    // `@posts = Post.all` and `@post = Post.find(...)` both read the posts table.
    let app = analyzed_app();
    let ctrl = &app.controllers[0];
    let posts_read = Effect::DbRead { table: TableRef(Symbol::from("posts")) };

    for action_name in ["index", "show", "destroy"] {
        let action = ctrl.actions().find(|a| a.name.as_str() == action_name).unwrap();
        assert!(
            action.effects.effects.contains(&posts_read),
            "{action_name} missing DbRead(posts); got {:?}",
            action.effects.effects
        );
    }
}

#[test]
fn destroy_effects_include_db_write_via_ivar_dispatch() {
    // `@post.destroy` — receiver is an Ivar bound to Post in a prior stmt.
    // The ivar's tracked type feeds into effect inference, producing a
    // DbWrite(posts) on the destroy site. Without ivar tracking, this
    // would fall through to Unknown and no write would be recorded.
    let app = analyzed_app();
    let destroy = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "destroy")
        .unwrap();
    let posts_write = Effect::DbWrite { table: TableRef(Symbol::from("posts")) };
    assert!(
        destroy.effects.effects.contains(&posts_write),
        "destroy missing DbWrite(posts); got {:?}",
        destroy.effects.effects
    );
}

#[test]
fn actions_without_db_calls_stay_pure() {
    // Not wired in the fixture, but we assert the negative: if a body does
    // nothing db-like, effects should be empty. Exercise with a hand-built
    // action via an empty body.
    use roundhouse::dialect::{Action, RenderTarget};
    use roundhouse::effect::EffectSet;
    use roundhouse::expr::Expr;
    use roundhouse::span::Span;
    use roundhouse::ty::Row;

    let empty_body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] });
    let mut action = Action {
        name: Symbol::from("noop"),
        params: Row::closed(),
        body: empty_body,
        renders: RenderTarget::Inferred,
        effects: EffectSet::singleton(Effect::Io), // seed a bogus effect
    };
    let analyzer = Analyzer::new(&analyzed_app());
    analyzer.analyze(&mut roundhouse::App::new()); // warm up is a no-op
    let body_ctx_effects = {
        // Simulate direct effect collection
        let mut app = roundhouse::App::new();
        app.controllers.push(roundhouse::dialect::Controller {
            name: ClassId(Symbol::from("NoopController")),
            parent: None,
            body: vec![roundhouse::ControllerBodyItem::Action {
                action: action.clone(),
                leading_comments: vec![],
                leading_blank_line: false,
            }],
        });
        analyzer.analyze(&mut app);
        app.controllers[0].actions().next().unwrap().effects.clone()
    };
    action.effects = body_ctx_effects;
    assert!(action.effects.effects.is_empty(), "expected empty effects for empty body");
}

// Per-expression effect annotation --------------------------------------
//
// These tests exercise the `expr.effects` field: the analyzer assigns
// each node its local side-effect contribution (typically non-empty only
// on Send nodes whose dispatched method is classified as effectful).
// The per-action aggregate (`action.effects`) must stay equal to the
// set-union of every node's local effects in the subtree — an invariant
// that gives future adapters/emitters a stable contract for reading
// effects off individual expressions.

#[test]
fn per_expr_effects_populated_on_send_site() {
    // `@posts = Post.all` — the inner Send carries DbRead(posts) as its
    // local effect; the Assign wrapper and the Const(Post) receiver are
    // pure, proving effects stay local to the dispatching node.
    let app = analyzed_app();
    let posts_read = Effect::DbRead { table: TableRef(Symbol::from("posts")) };

    let index = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "index")
        .unwrap();
    let ExprNode::Assign { value, .. } = &*index.body.node else { panic!() };
    assert!(
        value.effects.effects.contains(&posts_read),
        "Post.all Send should carry DbRead(posts); got {:?}",
        value.effects.effects,
    );
    assert!(
        index.body.effects.is_pure(),
        "Assign wrapper has no local effect; got {:?}",
        index.body.effects.effects,
    );
    let ExprNode::Send { recv, .. } = &*value.node else { panic!() };
    assert!(
        recv.as_ref().unwrap().effects.is_pure(),
        "Const(Post) receiver is pure",
    );
}

#[test]
fn per_expr_effects_on_instance_dispatch() {
    // `@post.destroy` — the Send carries DbWrite(posts) via the ivar
    // binding (analyzer tracked @post : Post from the prior Assign).
    // The receiver (Ivar read) is itself pure.
    let app = analyzed_app();
    let posts_write = Effect::DbWrite { table: TableRef(Symbol::from("posts")) };

    let destroy = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "destroy")
        .unwrap();
    let ExprNode::Seq { exprs } = &*destroy.body.node else { panic!() };
    // body: [find-assign, destroy, redirect]. Find the `@post.destroy` Send.
    let destroy_send = exprs
        .iter()
        .find(|e| matches!(
            &*e.node,
            ExprNode::Send { method, .. } if method.as_str() == "destroy"
        ))
        .expect("destroy Send present");
    assert!(
        destroy_send.effects.effects.contains(&posts_write),
        "@post.destroy should carry DbWrite(posts); got {:?}",
        destroy_send.effects.effects,
    );
    let ExprNode::Send { recv, .. } = &*destroy_send.node else { panic!() };
    assert!(
        recv.as_ref().unwrap().effects.is_pure(),
        "Ivar read is pure",
    );
}

#[test]
fn per_expr_effects_on_io_calls() {
    // `redirect_to posts_path` — classified as Io per the
    // ApplicationController synthetic-method effect table.
    let app = analyzed_app();
    let destroy = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "destroy")
        .unwrap();
    let ExprNode::Seq { exprs } = &*destroy.body.node else { panic!() };
    let redirect = exprs
        .iter()
        .find(|e| matches!(
            &*e.node,
            ExprNode::Send { method, .. } if method.as_str() == "redirect_to"
        ))
        .expect("redirect_to Send present");
    assert!(
        redirect.effects.effects.contains(&Effect::Io),
        "redirect_to should carry Io; got {:?}",
        redirect.effects.effects,
    );
}

#[test]
fn action_aggregate_equals_subtree_fold() {
    // Invariant: `action.effects` equals the set-union of every
    // per-expression `effects` in the action's body. The analyzer
    // computes both in one pass; if they diverge, per-expression
    // population is broken.
    use roundhouse::expr::{Expr, InterpPart};
    use std::collections::BTreeSet;

    fn fold(expr: &Expr, acc: &mut BTreeSet<Effect>) {
        acc.extend(expr.effects.effects.iter().cloned());
        match &*expr.node {
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. } => {}
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    fold(k, acc);
                    fold(v, acc);
                }
            }
            ExprNode::Array { elements, .. } => {
                for e in elements {
                    fold(e, acc);
                }
            }
            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let InterpPart::Expr { expr } = p {
                        fold(expr, acc);
                    }
                }
            }
            ExprNode::BoolOp { left, right, .. } => {
                fold(left, acc);
                fold(right, acc);
            }
            ExprNode::RescueModifier { expr, fallback } => {
                fold(expr, acc);
                fold(fallback, acc);
            }
            ExprNode::Let { value, body, .. } => {
                fold(value, acc);
                fold(body, acc);
            }
            ExprNode::Lambda { body, .. } => fold(body, acc),
            ExprNode::Apply { fun, args, block } => {
                fold(fun, acc);
                for a in args {
                    fold(a, acc);
                }
                if let Some(b) = block {
                    fold(b, acc);
                }
            }
            ExprNode::Send { recv, args, block, .. } => {
                if let Some(r) = recv {
                    fold(r, acc);
                }
                for a in args {
                    fold(a, acc);
                }
                if let Some(b) = block {
                    fold(b, acc);
                }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                fold(cond, acc);
                fold(then_branch, acc);
                fold(else_branch, acc);
            }
            ExprNode::Case { scrutinee, arms } => {
                fold(scrutinee, acc);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        fold(g, acc);
                    }
                    fold(&arm.body, acc);
                }
            }
            ExprNode::Seq { exprs } => {
                for e in exprs {
                    fold(e, acc);
                }
            }
            ExprNode::Assign { target, value } => {
                fold(value, acc);
                match target {
                    LValue::Attr { recv, .. } => fold(recv, acc),
                    LValue::Index { recv, index } => {
                        fold(recv, acc);
                        fold(index, acc);
                    }
                    _ => {}
                }
            }
            ExprNode::Yield { args } => {
                for a in args {
                    fold(a, acc);
                }
            }
            ExprNode::Raise { value } => fold(value, acc),
        }
    }

    let app = analyzed_app();
    for action in app.controllers[0].actions() {
        let mut folded: BTreeSet<Effect> = BTreeSet::new();
        fold(&action.body, &mut folded);
        assert_eq!(
            folded,
            action.effects.effects,
            "action {} — tree fold should equal the aggregate",
            action.name.as_str(),
        );
    }
}

// P1 — local variable tracking ------------------------------------------

/// Wraps a body expression in a minimal Controller/Action, runs the
/// analyzer, and returns the annotated body so tests can inspect types.
fn analyze_action_body(body: roundhouse::expr::Expr) -> roundhouse::expr::Expr {
    use roundhouse::dialect::{Action, Controller, RenderTarget};
    use roundhouse::effect::EffectSet;
    use roundhouse::ty::Row;
    use roundhouse::ControllerBodyItem;
    use std::collections::BTreeSet;

    let action = Action {
        name: Symbol::from("test_action"),
        params: Row::closed(),
        body,
        renders: RenderTarget::Inferred,
        effects: EffectSet { effects: BTreeSet::new() },
    };
    let mut app = roundhouse::App::new();
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("TestController")),
        parent: None,
        body: vec![ControllerBodyItem::Action {
            action,
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let ctrl = app.controllers.pop().unwrap();
    let item = ctrl.body.into_iter().next().unwrap();
    match item {
        ControllerBodyItem::Action { action, .. } => action.body,
        _ => panic!("expected Action"),
    }
}

#[test]
fn seq_local_assign_threads_forward() {
    // x = 5; x   -> Var lookup in stmt 2 finds x bound to Int in stmt 1.
    use roundhouse::expr::Expr;
    use roundhouse::ident::VarId;
    use roundhouse::span::Span;

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Seq {
            exprs: vec![
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Var { id: VarId(1), name: Symbol::from("x") },
                        value: Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Int { value: 5 } },
                        ),
                    },
                ),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: VarId(1), name: Symbol::from("x") },
                ),
            ],
        },
    );
    let analyzed = analyze_action_body(body);
    let ExprNode::Seq { exprs } = &*analyzed.node else { panic!() };
    let x_read = &exprs[1];
    assert_eq!(x_read.ty, Some(Ty::Int), "x in stmt 2 should be Int");
}

#[test]
fn array_each_block_param_types_as_element() {
    // [1].each { |n| n }   -> inside the block, Var n types as Int.
    use roundhouse::expr::Expr;
    use roundhouse::ident::VarId;
    use roundhouse::span::Span;

    let arr = Expr::new(
        Span::synthetic(),
        ExprNode::Array {
            elements: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 1 } },
            )],
            style: Default::default(),
        },
    );
    let block_body = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(1), name: Symbol::from("n") },
    );
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from("n")],
            block_param: None,
            body: block_body,
            block_style: Default::default(),
        },
    );
    let send = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(arr),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    let analyzed = analyze_action_body(send);
    let ExprNode::Send { block: Some(b), .. } = &*analyzed.node else { panic!() };
    let ExprNode::Lambda { body, .. } = &*b.node else { panic!() };
    assert_eq!(body.ty, Some(Ty::Int), "block param n should be Int inside body");
}

#[test]
fn hash_each_block_binds_key_and_value() {
    // {a: 1}.each { |k, v| v }   -> inside the block, v types as Int.
    use roundhouse::expr::Expr;
    use roundhouse::ident::VarId;
    use roundhouse::span::Span;

    let hash = Expr::new(
        Span::synthetic(),
        ExprNode::Hash {
            entries: vec![(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Sym { value: Symbol::from("a") } },
                ),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Int { value: 1 } },
                ),
            )],
            braced: true,
        },
    );
    let block_body = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(2), name: Symbol::from("v") },
    );
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from("k"), Symbol::from("v")],
            block_param: None,
            body: block_body,
            block_style: Default::default(),
        },
    );
    let send = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(hash),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    let analyzed = analyze_action_body(send);
    let ExprNode::Send { block: Some(b), .. } = &*analyzed.node else { panic!() };
    let ExprNode::Lambda { body, .. } = &*b.node else { panic!() };
    assert_eq!(body.ty, Some(Ty::Int), "block param v should be Int (hash value type)");
}

// P2 — controller→view ivar channel --------------------------------------

/// Walk an Expr collecting every `@ivar` read and its type.
fn collect_ivar_reads(expr: &roundhouse::expr::Expr, out: &mut Vec<(Symbol, Option<Ty>)>) {
    use roundhouse::expr::{ExprNode, InterpPart};
    match &*expr.node {
        ExprNode::Ivar { name } => {
            out.push((name.clone(), expr.ty.clone()));
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                collect_ivar_reads(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivar_reads(k, out);
                collect_ivar_reads(v, out);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivar_reads(r, out);
            }
            for a in args {
                collect_ivar_reads(a, out);
            }
            if let Some(b) = block {
                collect_ivar_reads(b, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_ivar_reads(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_ivar_reads(left, out);
            collect_ivar_reads(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_reads(cond, out);
            collect_ivar_reads(then_branch, out);
            collect_ivar_reads(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_ivar_reads(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_ivar_reads(g, out);
                }
                collect_ivar_reads(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivar_reads(value, out);
            collect_ivar_reads(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_ivar_reads(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_ivar_reads(fun, out);
            for a in args {
                collect_ivar_reads(a, out);
            }
            if let Some(b) = block {
                collect_ivar_reads(b, out);
            }
        }
        ExprNode::Assign { target, value } => {
            collect_ivar_reads(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_ivar_reads(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_ivar_reads(recv, out);
                collect_ivar_reads(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_ivar_reads(a, out);
            }
        }
        ExprNode::Raise { value } => collect_ivar_reads(value, out),
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Const { .. } => {}
    }
}

#[test]
fn articles_index_view_ivar_resolves_from_controller_action() {
    // Forcing function: ArticlesController#index binds `@articles = Article.includes(:comments).order(...)`,
    // which types as Array<Article>. The corresponding view `articles/index` should see
    // @articles pre-typed when its own body is analyzed, so `@articles.any?` in the ERB
    // dispatches against an Array — not Ty::Var(0).
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/index")
        .expect("articles/index view");

    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);

    let articles_reads: Vec<_> = reads
        .iter()
        .filter(|(n, _)| n.as_str() == "articles")
        .collect();
    assert!(
        !articles_reads.is_empty(),
        "articles/index should read @articles somewhere"
    );
    // Every @articles read should type as Array<Article>.
    for (_, ty) in &articles_reads {
        match ty {
            Some(Ty::Array { elem }) => match &**elem {
                Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Article"),
                other => panic!("expected Array<Article>, got Array<{other:?}>"),
            },
            other => panic!("expected @articles : Array<Article>, got {other:?}"),
        }
    }
}

// P2 — partial locals channel -------------------------------------------

/// Walk an Expr collecting every Var read and its type.
fn collect_var_reads(expr: &roundhouse::expr::Expr, out: &mut Vec<(Symbol, Option<Ty>)>) {
    use roundhouse::expr::{ExprNode, InterpPart};
    match &*expr.node {
        ExprNode::Var { name, .. } => {
            out.push((name.clone(), expr.ty.clone()));
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                collect_var_reads(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_var_reads(k, out);
                collect_var_reads(v, out);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_var_reads(r, out);
            }
            for a in args {
                collect_var_reads(a, out);
            }
            if let Some(b) = block {
                collect_var_reads(b, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_var_reads(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_var_reads(left, out);
            collect_var_reads(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_var_reads(cond, out);
            collect_var_reads(then_branch, out);
            collect_var_reads(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_var_reads(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_var_reads(g, out);
                }
                collect_var_reads(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_var_reads(value, out);
            collect_var_reads(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_var_reads(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_var_reads(fun, out);
            for a in args {
                collect_var_reads(a, out);
            }
            if let Some(b) = block {
                collect_var_reads(b, out);
            }
        }
        ExprNode::Assign { target, value } => {
            collect_var_reads(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_var_reads(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_var_reads(recv, out);
                collect_var_reads(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_var_reads(a, out);
            }
        }
        ExprNode::Raise { value } => collect_var_reads(value, out),
        ExprNode::Lit { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. } => {}
    }
}

/// Walk an Expr collecting every bare-name Send (no receiver, no args, no
/// block) with its type. In Ruby, `foo` without prior assignment is parsed
/// as `self.foo()` — the analyzer disambiguates at type time against
/// local_bindings, so this captures both local reads and true nullary
/// method calls.
fn collect_bare_name_sends(
    expr: &roundhouse::expr::Expr,
    out: &mut Vec<(Symbol, Option<Ty>)>,
) {
    use roundhouse::expr::{ExprNode, InterpPart};
    match &*expr.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if args.is_empty() && block.is_none() =>
        {
            out.push((method.clone(), expr.ty.clone()));
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_bare_name_sends(r, out);
            }
            for a in args {
                collect_bare_name_sends(a, out);
            }
            if let Some(b) = block {
                collect_bare_name_sends(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                collect_bare_name_sends(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_bare_name_sends(k, out);
                collect_bare_name_sends(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_bare_name_sends(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_bare_name_sends(left, out);
            collect_bare_name_sends(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_bare_name_sends(cond, out);
            collect_bare_name_sends(then_branch, out);
            collect_bare_name_sends(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_bare_name_sends(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_bare_name_sends(g, out);
                }
                collect_bare_name_sends(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_bare_name_sends(value, out);
            collect_bare_name_sends(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_bare_name_sends(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_bare_name_sends(fun, out);
            for a in args {
                collect_bare_name_sends(a, out);
            }
            if let Some(b) = block {
                collect_bare_name_sends(b, out);
            }
        }
        ExprNode::Assign { target, value } => {
            collect_bare_name_sends(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_bare_name_sends(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_bare_name_sends(recv, out);
                collect_bare_name_sends(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_bare_name_sends(a, out);
            }
        }
        ExprNode::Raise { value } => collect_bare_name_sends(value, out),
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. } => {}
    }
}

#[test]
fn article_partial_receives_article_local_from_collection_render() {
    // articles/index.html.erb contains `<%= render @articles %>`. With
    // @articles: Array<Article>, collection rendering dispatches to
    // articles/_article.html.erb binding local `article: Article`.
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let partial = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/_article")
        .expect("articles/_article partial");

    let mut sends = Vec::new();
    collect_bare_name_sends(&partial.body, &mut sends);
    let article_sends: Vec<_> =
        sends.iter().filter(|(n, _)| n.as_str() == "article").collect();
    assert!(
        !article_sends.is_empty(),
        "articles/_article body should reference local `article`"
    );
    for (_, ty) in &article_sends {
        match ty {
            Some(Ty::Class { id, .. }) => assert_eq!(id.0.as_str(), "Article"),
            other => panic!("expected article : Article, got {other:?}"),
        }
    }
}

#[test]
fn form_partial_receives_article_local_from_named_render() {
    // articles/new.html.erb contains `<%= render "form", article: @article %>`.
    // @article: Article in the new action, so local `article: Article` should
    // flow into articles/_form.html.erb.
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let partial = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/_form")
        .expect("articles/_form partial");

    let mut sends = Vec::new();
    collect_bare_name_sends(&partial.body, &mut sends);
    let article_sends: Vec<_> =
        sends.iter().filter(|(n, _)| n.as_str() == "article").collect();
    assert!(
        !article_sends.is_empty(),
        "articles/_form body should reference local `article`"
    );
    let any_typed_as_article = article_sends.iter().any(|(_, ty)| {
        matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "Article")
    });
    assert!(
        any_typed_as_article,
        "at least one `article` read in articles/_form should type as Article; got {:?}",
        article_sends
            .iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
    );
}

#[test]
fn new_view_sees_article_from_new_action() {
    // ArticlesController#new binds `@article = Article.new`, type Article.
    // articles/new.html.erb references @article (in `render "form", article: @article`).
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/new")
        .expect("articles/new view");

    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);

    let article_reads: Vec<_> = reads
        .iter()
        .filter(|(n, _)| n.as_str() == "article")
        .collect();
    assert!(
        !article_reads.is_empty(),
        "articles/new should read @article"
    );
    for (_, ty) in &article_reads {
        match ty {
            Some(Ty::Class { id, .. }) => assert_eq!(id.0.as_str(), "Article"),
            other => panic!("expected @article : Article, got {other:?}"),
        }
    }
}

#[test]
fn let_body_sees_bound_name() {
    // Let { name: x, value: 5, body: x }   -> body types as Int.
    use roundhouse::expr::Expr;
    use roundhouse::ident::VarId;
    use roundhouse::span::Span;

    let let_expr = Expr::new(
        Span::synthetic(),
        ExprNode::Let {
            id: VarId(1),
            name: Symbol::from("x"),
            value: Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 5 } },
            ),
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(1), name: Symbol::from("x") },
            ),
        },
    );
    let analyzed = analyze_action_body(let_expr);
    assert_eq!(analyzed.ty, Some(Ty::Int), "Let body should resolve x to Int");
}

// Diagnostics -------------------------------------------------------------

#[test]
fn before_action_seeds_dependent_action_ctx() {
    // `before_action :set_article, only: %i[show edit update destroy]` in
    // ArticlesController binds @article before the body of each listed
    // action runs. Verify that the `update` action (which reads @article
    // via `@article.update(article_params)`) sees @article typed as
    // Article — not Ty::Var(0) — even though its body doesn't assign it.
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let ctrl = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ArticlesController")
        .expect("ArticlesController");
    let update = ctrl.actions().find(|a| a.name.as_str() == "update").expect("update");

    let mut reads = Vec::new();
    collect_ivar_reads(&update.body, &mut reads);
    let article_reads: Vec<_> =
        reads.iter().filter(|(n, _)| n.as_str() == "article").collect();
    assert!(
        !article_reads.is_empty(),
        "update action should read @article"
    );
    for (_, ty) in &article_reads {
        match ty {
            Some(Ty::Class { id, .. }) => assert_eq!(id.0.as_str(), "Article"),
            other => panic!("expected @article : Article via before_action, got {other:?}"),
        }
    }
}

#[test]
fn before_action_propagates_through_to_view() {
    // articles/show.html.erb references @article. The `show` action body
    // is empty — @article only exists in that action because of
    // `before_action :set_article`. The action→view ivar channel should
    // therefore deliver @article: Article into the view.
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/show")
        .expect("articles/show view");

    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);
    let article_reads: Vec<_> =
        reads.iter().filter(|(n, _)| n.as_str() == "article").collect();
    assert!(
        !article_reads.is_empty(),
        "articles/show should read @article"
    );
    // Not every read needs to be Article (ERB's `_buf + x.to_s` can leave
    // unions or intermediaries), but at least one should be.
    let any_article = article_reads.iter().any(|(_, ty)| {
        matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "Article")
    });
    assert!(
        any_article,
        "at least one @article read in articles/show should type as Article; got {:?}",
        article_reads.iter().map(|(_, t)| t).collect::<Vec<_>>()
    );
}

#[test]
fn diagnose_flags_send_dispatch_failure_on_known_receiver() {
    // Construct a synthetic action body that calls a method the registry
    // doesn't know on a receiver whose type is resolved. Diagnose should
    // pick up exactly that Send. Keeps coverage on the SendDispatchFailed
    // path without depending on real-fixture gaps (which we're closing).
    use roundhouse::expr::Expr;
    use roundhouse::span::Span;

    // `Post.frobnicate` — Post is a known model class in tiny-blog, but
    // `frobnicate` is not in any registry.
    let frobnicate = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("Post")] },
            )),
            method: Symbol::from("frobnicate"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );

    // Use tiny-blog as the surrounding app so `Post` is a known class.
    let mut app = ingest_app(fixture_path()).expect("ingest");
    // Splice the synthetic expression into an existing action's body.
    let ctrl = &mut app.controllers[0];
    let action = ctrl.actions_mut().next().expect("at least one action");
    let original_body = std::mem::replace(
        &mut action.body,
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    );
    action.body = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![original_body, frobnicate] },
    );

    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);

    let frob: Vec<_> = diags
        .iter()
        .filter(|d| matches!(
            &d.kind,
            DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "frobnicate"
        ))
        .collect();
    assert_eq!(
        frob.len(),
        1,
        "expected exactly one SendDispatchFailed for Post.frobnicate; got {:?}",
        diags,
    );
}

#[test]
fn diagnose_is_silent_on_tiny_blog() {
    // Tiny-blog's full surface — controllers, scopes, methods, and the
    // ERB index view — should type with zero diagnostics after:
    //   • P1 (local Var + block params)
    //   • P2 (controller→view channel + partial locals)
    //   • AR instance-method seeding (save/destroy/update/etc.)
    //   • Str/Int operator entries (handles the ERB-compiled `_buf + "..."`)
    //
    // If this starts failing, the delta lists the new gap — treat it as
    // a signal to extend the registry rather than loosen the assertion.
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);
    assert!(
        diags.is_empty(),
        "tiny-blog should produce zero diagnostics; got {:#?}",
        diags,
    );
}

#[test]
fn analysis_is_idempotent() {
    // Running the analyzer twice should produce identical results.
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let first = app.clone();
    Analyzer::new(&app).analyze(&mut app);
    assert_eq!(first, app, "analyzer must be idempotent");
}

#[test]
fn ruby_emit_is_type_invariant() {
    // Ruby is dynamic: `a + b` dispatches at runtime, so the Ruby emitter
    // never needs types to pick an operation. Running the analyzer must
    // therefore leave Ruby emission unchanged.
    //
    // This test is SPECIFIC to the Ruby emitter. Typed targets (Rust, Go,
    // typed TypeScript) will emit differently depending on operand types —
    // that's the whole point of the type system. Do NOT generalize this
    // assertion across emitters.
    use roundhouse::emit::ruby;
    let unanalyzed = ingest_app(fixture_path()).expect("ingest");
    let mut analyzed = unanalyzed.clone();
    Analyzer::new(&analyzed).analyze(&mut analyzed);
    assert_eq!(ruby::emit(&unanalyzed), ruby::emit(&analyzed));
}
