//! Analyzer smoke test: types land on expressions we can verify.
//!
//! Keep these tests specific — pick a location in the IR that has an
//! unambiguous expected type, and assert it. Broader coverage goes in
//! snapshot tests once we have them.

use std::path::Path;

use roundhouse::analyze::Analyzer;
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
            }],
        });
        analyzer.analyze(&mut app);
        app.controllers[0].actions().next().unwrap().effects.clone()
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
