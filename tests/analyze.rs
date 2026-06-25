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
use roundhouse::{ClassId, RenderTarget, Symbol, TableRef};

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
    // The `published: true` kwarg is a Hash literal (kwargs: true in IR).
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
        ExprNode::Hash { entries, kwargs } => {
            assert!(*kwargs, "trailing-kwargs form should set kwargs=true");
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
fn builder_chain_sends_do_not_carry_db_effects() {
    // `scope :published, -> { where(published: true) }` — the
    // top-level Send is `where(published: true)`, a Relation-
    // builder call on implicit self. Under the catalog's
    // `ChainKind::Builder` gating, this Send should carry NO
    // DbRead effect — the Relation is lazy; only a Terminal call
    // (`.all`, `.first`, `.to_a`) actually executes the query
    // and attaches the effect.
    //
    // Consequence for async emission: async-capable emitters
    // don't emit `await` at Builder sites, avoiding spurious
    // round-trips per chain link. Only the terminal step awaits.
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
    // Body: `where(published: true)` — Send. Local effects must
    // be empty.
    assert!(
        published.body.effects.is_pure(),
        "Builder Send `where(...)` should carry no effects; got {:?}",
        published.body.effects.effects,
    );
}

#[test]
fn terminal_sends_still_carry_db_effects() {
    // `scope :recent, -> { limit(10) }` — `limit` is catalog-
    // classified as Builder, so its local effect is empty (new
    // behavior). Contrast with `Post.all` in an action body,
    // which is Terminal and retains DbRead(posts).
    let app = analyzed_app();
    let posts_read = Effect::DbRead {
        table: TableRef(Symbol::from("posts")),
    };
    let index = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "index")
        .unwrap();
    // Body: `@posts = Post.all`
    let ExprNode::Assign { value, .. } = &*index.body.node else {
        panic!("expected Assign at index top");
    };
    // `value` is the `Post.all` Send — Terminal, should carry
    // DbRead(posts) as before.
    assert!(
        value.effects.effects.contains(&posts_read),
        "Terminal Send `Post.all` should carry DbRead(posts); got {:?}",
        value.effects.effects,
    );
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
fn custom_adapter_suppresses_db_effects() {
    // Proves `Analyzer::with_adapter` actually threads through to
    // effect inference: swap in an adapter that returns Unknown for
    // every AR method, analyze the same fixture, and confirm no
    // DbRead/DbWrite effects land anywhere. The Io effects from
    // render/redirect_to still appear — those are Rails-dialect, not
    // adapter territory.
    use roundhouse::adapter::{ArMethodKind, DatabaseAdapter};

    struct NoDbAdapter;
    impl DatabaseAdapter for NoDbAdapter {
        fn classify_ar_method(&self, _method: &str) -> ArMethodKind {
            ArMethodKind::Unknown
        }
    }

    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::with_adapter(&app, Box::new(NoDbAdapter)).analyze(&mut app);

    for action in app.controllers[0].actions() {
        for e in &action.effects.effects {
            match e {
                Effect::DbRead { .. } | Effect::DbWrite { .. } => {
                    panic!(
                        "NoDbAdapter should have suppressed DB effects; {} carries {:?}",
                        action.name.as_str(),
                        e,
                    );
                }
                _ => {}
            }
        }
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
    let mut analyzer = Analyzer::new(&analyzed_app());
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
            layout: Default::default(),
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
            | ExprNode::Const { .. }
            | ExprNode::Retry
            | ExprNode::Redo
            | ExprNode::SelfRef => {}
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
            ExprNode::Assign { target, value }
            | ExprNode::OpAssign { target, value, .. } => {
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
            ExprNode::Return { value } => fold(value, acc),
            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args {
                        fold(a, acc);
                    }
                }
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                fold(body, acc);
                for r in rescues {
                    for c in &r.classes {
                        fold(c, acc);
                    }
                    fold(&r.body, acc);
                }
                if let Some(e) = else_branch {
                    fold(e, acc);
                }
                if let Some(e) = ensure {
                    fold(e, acc);
                }
            }
            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { fold(v, acc); }
            }
            ExprNode::Splat { value } => fold(value, acc),
            ExprNode::MultiAssign { value, .. } => fold(value, acc),
            ExprNode::While { cond, body, .. } => {
                fold(cond, acc);
                fold(body, acc);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { fold(b, acc); }
                if let Some(e) = end { fold(e, acc); }
            }
            ExprNode::Cast { value, .. } => fold(value, acc),
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
        layout: Default::default(),
    });
    let mut analyzer = Analyzer::new(&app);
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
            kwargs: false,
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
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
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
        ExprNode::Return { value } => collect_ivar_reads(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_ivar_reads(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_ivar_reads(body, out);
            for r in rescues {
                for c in &r.classes {
                    collect_ivar_reads(c, out);
                }
                collect_ivar_reads(&r.body, out);
            }
            if let Some(e) = else_branch {
                collect_ivar_reads(e, out);
            }
            if let Some(e) = ensure {
                collect_ivar_reads(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { collect_ivar_reads(v, out); }
        }
        ExprNode::Splat { value } => collect_ivar_reads(value, out),
        ExprNode::MultiAssign { value, .. } => collect_ivar_reads(value, out),
        ExprNode::While { cond, body, .. } => {
            collect_ivar_reads(cond, out);
            collect_ivar_reads(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { collect_ivar_reads(b, out); }
            if let Some(e) = end { collect_ivar_reads(e, out); }
        }
        ExprNode::Cast { value, .. } => collect_ivar_reads(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
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
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
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
        ExprNode::Return { value } => collect_var_reads(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_var_reads(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_var_reads(body, out);
            for r in rescues {
                for c in &r.classes {
                    collect_var_reads(c, out);
                }
                collect_var_reads(&r.body, out);
            }
            if let Some(e) = else_branch {
                collect_var_reads(e, out);
            }
            if let Some(e) = ensure {
                collect_var_reads(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { collect_var_reads(v, out); }
        }
        ExprNode::Splat { value } => collect_var_reads(value, out),
        ExprNode::MultiAssign { value, .. } => collect_var_reads(value, out),
        ExprNode::While { cond, body, .. } => {
            collect_var_reads(cond, out);
            collect_var_reads(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { collect_var_reads(b, out); }
            if let Some(e) = end { collect_var_reads(e, out); }
        }
        ExprNode::Cast { value, .. } => collect_var_reads(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
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
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
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
        ExprNode::Return { value } => collect_bare_name_sends(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_bare_name_sends(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_bare_name_sends(body, out);
            for r in rescues {
                for c in &r.classes {
                    collect_bare_name_sends(c, out);
                }
                collect_bare_name_sends(&r.body, out);
            }
            if let Some(e) = else_branch {
                collect_bare_name_sends(e, out);
            }
            if let Some(e) = ensure {
                collect_bare_name_sends(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { collect_bare_name_sends(v, out); }
        }
        ExprNode::Splat { value } => collect_bare_name_sends(value, out),
        ExprNode::MultiAssign { value, .. } => collect_bare_name_sends(value, out),
        ExprNode::While { cond, body, .. } => {
            collect_bare_name_sends(cond, out);
            collect_bare_name_sends(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { collect_bare_name_sends(b, out); }
            if let Some(e) = end { collect_bare_name_sends(e, out); }
        }
        ExprNode::Cast { value, .. } => collect_bare_name_sends(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
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
    // ERB index view — types with ZERO diagnostics: no errors, and (since
    // the route/view helpers it uses like `posts_path` are now modeled) no
    // coverage-class warnings either. Re-tightened to fully-clean after the
    // view-helper catalog landed; it had briefly relaxed to "zero errors"
    // while `unresolved_type` surfaced the unmodeled helpers.
    //
    // If a diagnostic appears here, the delta lists the new gap — extend
    // the registry rather than loosen the assertion.
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

// before_action filter ivar seeding -------------------------------------

/// Ingest + analyze a hand-built in-memory app tree.
fn app_from_files(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Names of every `@ivar` the analyzer couldn't bind a type for.
fn ivar_unresolved_names(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::IvarUnresolved { name } => Some(name.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// Method names that failed dispatch on a known receiver type.
fn send_dispatch_failures(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => {
                Some(method.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn association_writers_and_ar_instance_methods_resolve() {
    // belongs_to/has_one register a writer `name=` (not just the reader),
    // and the AR Dirty/persistence instance methods missing from the
    // catalog (`update_column`, `marked_for_destruction?`, …) resolve on a
    // model instance.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  belongs_to :owner
  has_many :parts

  def reassign(o, list)
    self.owner = o
    self.parts = list
    self.update_column(:name, "x")
    self.marked_for_destruction?
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in ["owner=", "parts=", "update_column", "marked_for_destruction?"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "`{m}` should resolve on a model instance; dispatch failures = {failures:?}"
        );
    }
}

#[test]
fn cross_class_constants_resolve_by_value() {
    // A constant declared on one class (`Vote::COMMENT_REASONS = {…}.freeze`)
    // must resolve to its *value* type — Hash / Range / Int — when
    // referenced from another class, not the `Ty::Class { id: ConstName }`
    // fallback. Exercises the whole chain: `.freeze` identity, the
    // merge-dependency fixpoint (`ALL = BASE.merge(…)`), Hash `[]`, Range
    // `.include?`, and `Int > Int` (no incompatible_binop).
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/vote.rb",
            r#"class Vote < ApplicationRecord
  COMMENT_REASONS = { "O" => "Off-topic" }.freeze
  ALL_COMMENT_REASONS = COMMENT_REASONS.merge({ "I" => "Incorrect" }).freeze
  SCORE_RANGE = (-2..4).freeze
  MIN_DAYS = 90
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/votes_controller.rb",
            r#"class VotesController < ApplicationController
  def show
    @a = Vote::COMMENT_REASONS["O"]
    @b = Vote::ALL_COMMENT_REASONS["I"]
    @c = Vote::SCORE_RANGE.include?(1)
    @d = (5 > Vote::MIN_DAYS)
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in ["[]", "include?"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "constant-by-value should resolve `{m}` cross-class; failures = {failures:?}"
        );
    }
    let binops = diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .count();
    assert_eq!(binops, 0, "`Int > Vote::MIN_DAYS` is Int > Int — must not flag");
}

#[test]
fn stdlib_singletons_and_set_resolve() {
    // The hardcoded Ruby stdlib catalog (SecureRandom, CGI, Digest::*,
    // Math, File, Dir, Set) resolves the common call surface, and unary
    // minus (`-x` → `x.-@`) dispatches on the now-concrete numeric.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/thing.rb",
            r#"class Thing < ApplicationRecord
  def compute
    token = SecureRandom.hex(8)
    safe = CGI.escape(token)
    digest = Digest::MD5.hexdigest(safe)
    root = Math.sqrt(4.0)
    files = Dir.entries("/tmp")
    seen = Set.new
    seen << digest
    seen.each { |x| x }
    score = -((root * 2.0).round(3))
    [token, safe, digest, files, score]
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in [
        "hex", "escape", "hexdigest", "sqrt", "entries", "<<", "each", "-@",
    ] {
        assert!(
            !failures.iter().any(|f| f == m),
            "stdlib `{m}` should resolve via the hardcoded catalog; failures = {failures:?}"
        );
    }
}

#[test]
fn create_view_columns_register_with_real_schema_types() {
    // A model backed by a SQL `create_view` gets its columns from the
    // SELECT `AS <alias>` list. A direct `table.column` projection
    // resolves to that column's REAL type from the already-parsed
    // schema; computed columns (comparisons, subqueries — lobsters'
    // current_vote_*/is_unread) fall back to a name heuristic. Both
    // resolve as model attributes.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "comments", force: :cascade do |t|
    t.integer "score"
  end
  create_view "scored_comments", sql_definition: <<-SQL
      select `comments`.`score` AS `tally`,
        (`a` < `b`) AS `is_flagged`,
        (select `v`.`vote` from `votes` `v`) AS `current_vote_vote`
      from `comments`
  SQL
end
"#,
        ),
        (
            "app/models/scored_comment.rb",
            r#"class ScoredComment < ApplicationRecord
  def check
    [self.tally.zero?, self.is_flagged, self.current_vote_vote]
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    // `tally` is a direct projection of comments.score (integer), so
    // schema lookup gives Int → `.zero?` resolves. If it had fallen
    // back to the String heuristic, `zero?` would fail on Str.
    assert!(
        !failures.iter().any(|f| f == "tally" || f == "zero?"),
        "direct-projection view column should resolve to its real Int \
         type (zero? proves it); failures = {failures:?}"
    );
    // Computed columns (no single source column) still resolve via the
    // name-heuristic fallback.
    for m in ["is_flagged", "current_vote_vote"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "computed view column `{m}` should resolve via fallback; \
             failures = {failures:?}"
        );
    }
}

#[test]
fn has_secure_password_and_update_counters_resolve() {
    // `has_secure_password` generates `password=`/`password_confirmation=`
    // writers and `authenticate`; `Model.update_counters(id, col: n)` is
    // an AR class method (atomic counter bump → Int). Both used to fail
    // dispatch.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_secure_password
  def reset
    self.password = "x"
    self.password_confirmation = "x"
    User.update_counters(self.id, karma: 1)
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    for m in ["password=", "password_confirmation=", "update_counters"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "`{m}` should resolve (has_secure_password / AR catalog); \
             failures = {failures:?}"
        );
    }
}

#[test]
fn bare_module_under_app_models_registers_as_library_class() {
    // A bare `module Foo; def self.x; …` under app/models/ (e.g.
    // lobsters' InactiveUser) is a namespace of singleton methods, not
    // a model. It used to classify as None → ingest_model → dropped, so
    // `Foo.x` failed dispatch. Now it ingests as a library class and the
    // `def self.x` resolve as dotted-call class methods.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/inactive_user.rb",
            r#"module InactiveUser
  def self.label
    "inactive"
  end
  def self.disown!(x)
    x
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def caption
    InactiveUser.label.upcase
  end
  def drop(c)
    InactiveUser.disown!(c)
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    for m in ["label", "disown!", "upcase"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "`InactiveUser.{m}` (module singleton method) should resolve; \
             failures = {failures:?}"
        );
    }
}

#[test]
fn send_dispatches_on_known_receiver() {
    // Reflective `send` on a known receiver resolves, not "no known
    // method send". A LITERAL symbol arg dispatches the named method
    // exactly (`self.send(:title)` → the title reader). A DYNAMIC arg
    // (`self.send(k)` in an as_json loop) is bounded by the receiver's
    // method-return union, which absorbs to Untyped — either way no
    // send_dispatch_failed.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "posts", force: :cascade do |t|
    t.string "title"
  end
end
"#,
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  def shout
    self.send(:title).upcase
  end
  def dump(keys)
    js = {}
    keys.each do |k|
      js[k] = self.send(k)
    end
    js
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    // `send` itself always resolves now.
    assert!(
        !failures.iter().any(|f| f == "send"),
        "`send` should resolve on a known receiver; failures = {failures:?}"
    );
    // Tier 1: literal `send(:title)` → Str, so `.upcase` resolves too.
    assert!(
        !failures.iter().any(|f| f == "upcase"),
        "`self.send(:title).upcase` should resolve via literal dispatch; \
         failures = {failures:?}"
    );
}

#[test]
fn rails_env_is_a_string_inquirer() {
    // `Rails.env` is an ActiveSupport::StringInquirer: `development?` /
    // `production?` (any `<word>?`) resolve to Bool via method_missing,
    // and it's otherwise a String (`==`/`upcase`/`to_sym`). It used to
    // type as plain Str and reject the env predicates.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  def check
    a = Rails.env.development?
    b = Rails.env.production?
    c = Rails.env.upcase
    [a, b, c]
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    for m in ["development?", "production?", "upcase"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "`Rails.env.{m}` should resolve (StringInquirer is a String \
             that answers `?` inquiries); failures = {failures:?}"
        );
    }
}

#[test]
fn hash_accumulator_value_widens_from_writes() {
    // The `hash[k] ||= []; hash[k].push x` accumulator idiom: an empty
    // `{}` seeds the value type as Var, but `hash[k] ||= []` widens it
    // to Array, so the following `hash[k].push` resolves (it used to
    // dispatch `push` on the `hash[k]` read type `Var|Nil`).
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  def grouped(items)
    h = {}
    items.each do |x|
      h[x.k] ||= []
      h[x.k].push(x)
    end
    h
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    assert!(
        !failures.iter().any(|f| f == "push"),
        "hash[k].push should resolve after `hash[k] ||= []` widens the \
         value to Array; failures = {failures:?}"
    );
}

#[test]
fn diverging_tail_method_harvests_early_returns() {
    // A method whose *tail* diverges (here a `raise`; in lobsters a
    // `begin/case` whose arms all `return`) but which returns a
    // concrete value on an early path must harvest that early return's
    // type — not `Bottom`. Without it, a caller's `lookup["k"]` fails
    // dispatch on `Bottom`.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  def lookup
    return({ "found" => "yes" }) if @ready
    raise "not ready"
  end
  def use
    lookup["found"]
  end
end
"#,
        ),
    ]);
    let failures = send_dispatch_failures(&app);
    assert!(
        !failures.iter().any(|f| f == "[]"),
        "`lookup[...]` should resolve — lookup returns Hash via its early \
         return, not Bottom from the raising tail; failures = {failures:?}"
    );
}

#[test]
fn datetime_columns_type_as_time() {
    // A schema datetime column is a `Time` at the Ruby level, so
    // `created_at.strftime` / `.to_i` / `.after?` / `>=` all resolve.
    // It used to mis-type as `Str` (a runtime-storage detail that
    // belongs on the emit side) and reject every Time method. The
    // chained `.to_i` result feeds Int arithmetic, proving the real
    // type, not a gradual escape.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "posts", force: :cascade do |t|
    t.string "title"
    t.datetime "created_at", null: false
    t.datetime "published_at"
  end
end
"#,
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  def stamps
    label = self.created_at.strftime("%Y-%m-%d")
    epoch = self.created_at.to_i + 1
    fresh = self.created_at.after?(self.published_at)
    recent = self.created_at >= self.published_at
    [label, epoch, fresh, recent]
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in ["strftime", "to_i", "after?", ">=", "+"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "Time method `{m}` should resolve on a datetime column; failures = {failures:?}"
        );
    }
}

#[test]
fn gem_catalog_resolves_third_party_surface() {
    // The gem catalog (src/catalog/gems.rs) resolves the third-party
    // surface apps call: class methods (`Arel.sql`, `ROTP::Base32.random`),
    // instance methods reached through the universal `.new`
    // (`ROTP::TOTP.new.secret`, `Mail::Address.new(x).domain`), and
    // module methods (`Nokogiri::HTML`). `.random`/`.secret` carry a
    // real `Str` type, not just `Untyped`, so a chained String method
    // resolves too — `random.upcase` would fail "no known method on
    // Untyped"... no, Untyped absorbs; it would fail on a *Var*. We
    // assert the gem methods AND the chained `upcase` all resolve.
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/thing.rb",
            r#"class Thing < ApplicationRecord
  def compute
    frag = Arel.sql("a = b")
    secret = ROTP::Base32.random
    loud = secret.upcase
    totp = ROTP::TOTP.new(secret)
    uri = totp.provisioning_uri("x")
    doc = Nokogiri::HTML("<p>")
    addr = Mail::Address.new("a@b.com").domain
    [frag, secret, loud, uri, doc, addr]
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in [
        "sql", "random", "upcase", "provisioning_uri", "HTML", "domain",
    ] {
        assert!(
            !failures.iter().any(|f| f == m),
            "gem method `{m}` should resolve via the gem catalog; failures = {failures:?}"
        );
    }
}

#[test]
fn app_helper_module_singletons_resolve() {
    // Helper modules under app/helpers/ are walked as library classes, so a
    // helper called as a bare singleton (`TrafficHelper.novelty_logo`) — its
    // methods declared `def self.x` — dispatches against the registered
    // module instead of failing "no known method on Class { TrafficHelper }".
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    @logo = TrafficHelper.novelty_logo
  end
end
"#,
        ),
        (
            "app/helpers/traffic_helper.rb",
            r#"module TrafficHelper
  def self.novelty_logo
    nil
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    assert!(
        !failures.iter().any(|f| f == "novelty_logo"),
        "`TrafficHelper.novelty_logo` should resolve once app/helpers is \
         walked; dispatch failures = {failures:?}"
    );
}

#[test]
fn multi_symbol_before_action_seeds_every_target() {
    // `before_action :load_user, :load_widget` declares two filters on one
    // line. The old single-target parse captured only `:load_user`, so
    // @widget_count — set solely by `load_widget` — never reached the
    // `show` view. Both targets must seed now.
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            r#"class ThingsController < ApplicationController
  before_action :load_user, :load_widget

  def show
  end

  private

  def load_user
    @user_name = "alice"
  end

  def load_widget
    @widget_count = 7
  end
end
"#,
        ),
        (
            "app/views/things/show.html.erb",
            "<p><%= @user_name %></p>\n<p><%= @widget_count %></p>\n",
        ),
    ]);

    let unresolved = ivar_unresolved_names(&app);
    assert!(
        !unresolved.iter().any(|n| n == "widget_count"),
        "@widget_count (the dropped 2nd before_action target) should resolve; \
         unresolved = {unresolved:?}"
    );
    assert!(
        !unresolved.iter().any(|n| n == "user_name"),
        "@user_name (1st before_action target) should resolve; unresolved = {unresolved:?}"
    );
}

#[test]
fn block_form_before_action_seeds_ivars() {
    // A block filter `before_action { @count = 5 }` names no method, so it
    // survives ingest as an `Unknown` body item rather than a `Filter`. Its
    // ivar must still seed the guarded actions and their views.
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action { @count = 5 }

  def index
  end
end
"#,
        ),
        ("app/views/widgets/index.html.erb", "<p><%= @count %></p>\n"),
    ]);

    let unresolved = ivar_unresolved_names(&app);
    assert!(
        !unresolved.iter().any(|n| n == "count"),
        "@count (set by the block-form before_action) should resolve; \
         unresolved = {unresolved:?}"
    );

    // And it should carry the concrete literal type, not just "present".
    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "widgets/index")
        .expect("widgets/index view");
    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);
    assert!(
        reads
            .iter()
            .any(|(n, ty)| n.as_str() == "count" && matches!(ty, Some(Ty::Int))),
        "@count should read as Int in the view; got {:?}",
        reads.iter().filter(|(n, _)| n.as_str() == "count").collect::<Vec<_>>()
    );
}

#[test]
fn explicit_render_template_binds_view_and_skips_respond_to() {
    // `reused` renders `:show` at the top level — the "this action reuses
    // another action's template" idiom — so its view is `things/show`, not
    // the convention `things/reused`. `formatted` only renders inside a
    // `respond_to` block (where each MIME type names its own template), so
    // it must keep its convention view rather than mis-binding to `new`.
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            r#"class ThingsController < ApplicationController
  def reused
    @greeting = "hi"
    render :show
  end

  def formatted
    respond_to do |format|
      format.html { render :new }
      format.json { render :show }
    end
  end
end
"#,
        ),
        ("app/views/things/show.html.erb", "<p><%= @greeting %></p>\n"),
    ]);

    let ctrl = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("ThingsController");

    let reused = ctrl.actions().find(|a| a.name.as_str() == "reused").expect("reused");
    assert!(
        matches!(&reused.renders, RenderTarget::Template { name, .. } if name.as_str() == "show"),
        "top-level `render :show` should set Template{{show}}; got {:?}",
        reused.renders
    );

    // Safety: respond_to-only renders stay Inferred (this is what keeps
    // real-blog's multi-format create/update at 0/0).
    let formatted = ctrl.actions().find(|a| a.name.as_str() == "formatted").expect("formatted");
    assert!(
        matches!(formatted.renders, RenderTarget::Inferred),
        "respond_to-nested renders must stay Inferred; got {:?}",
        formatted.renders
    );

    // The reused template's view resolves @greeting (set only by `reused`).
    let unresolved = ivar_unresolved_names(&app);
    assert!(
        !unresolved.iter().any(|n| n == "greeting"),
        "@greeting should resolve in things/show via `render :show`; unresolved = {unresolved:?}"
    );
}


#[test]
fn mailer_actions_dispatch_on_the_class_and_chain_deliver() {
    // An ActionMailer subclass declares its actions as plain *instance*
    // `def`s but Rails invokes them on the *class*, returning a deliverable:
    //   `Notifier.welcome(user).deliver_now`
    // The mailer ingests as a library class (parent → ActionMailer::Base);
    // analyze re-exposes each public action as a class method returning
    // `ActionMailer::MessageDelivery`, whose `deliver_*` methods resolve so
    // the whole chain types. None of `welcome` / `deliver_now` /
    // `deliver_later` should hit "no known method".
    let app = app_from_files(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/mailers/application_mailer.rb",
            "class ApplicationMailer < ActionMailer::Base\nend\n",
        ),
        (
            "app/mailers/notifier.rb",
            r#"class Notifier < ApplicationMailer
  def welcome(user)
    @user = user
    mail(:to => user.email, :subject => "hi")
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def announce
    Notifier.welcome(self).deliver_now
    Notifier.welcome(self).deliver_later
  end
end
"#,
        ),
    ]);

    let failures = send_dispatch_failures(&app);
    for m in ["welcome", "deliver_now", "deliver_later"] {
        assert!(
            !failures.iter().any(|f| f == m),
            "mailer chain `{m}` should resolve; dispatch failures = {failures:?}"
        );
    }
}
