//! Analyzer smoke test: types land on expressions we can verify.
//!
//! Keep these tests specific — pick a location in the IR that has an
//! unambiguous expected type, and assert it. Broader coverage goes in
//! snapshot tests once we have them.

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::effect::Effect;
use roundhouse::expr::{ExprNode, LValue};
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
        .actions
        .iter()
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
        .actions
        .iter()
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
    let scope = &post_model.scopes[0];
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
        .actions
        .iter()
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
    let index = &app.controllers[0].actions[0];
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
        .actions
        .iter()
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
    let scope = &post.scopes[0];
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
fn action_effects_include_db_reads() {
    // `@posts = Post.all` and `@post = Post.find(...)` both read the posts table.
    let app = analyzed_app();
    let ctrl = &app.controllers[0];
    let posts_tab = Effect::DbRead { table: TableRef(Symbol::from("posts")) };

    for action_name in ["index", "show"] {
        let action = ctrl.actions.iter().find(|a| a.name.as_str() == action_name).unwrap();
        assert!(
            action.effects.effects.contains(&posts_tab),
            "{action_name} missing DbRead(posts); got {:?}",
            action.effects.effects
        );
    }
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
            filters: vec![],
            actions: vec![action.clone()],
        });
        analyzer.analyze(&mut app);
        app.controllers[0].actions[0].effects.clone()
    };
    action.effects = body_ctx_effects;
    assert!(action.effects.effects.is_empty(), "expected empty effects for empty body");
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
