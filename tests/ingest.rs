//! Ingest smoke test: reading fixtures/tiny-blog/ produces the expected IR.

use std::path::Path;

use roundhouse::dialect::{Association, CallbackHook, Dependent, ValidationRule};
use roundhouse::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use roundhouse::ingest::ingest_app;
use roundhouse::schema::ColumnType;
use roundhouse::{HttpMethod, RenderTarget};

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

#[test]
fn ingests_schema_tables_and_columns() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.schema.tables.len(), 2, "expected posts and comments");

    let posts = app
        .schema
        .tables
        .get(&roundhouse::Symbol::from("posts"))
        .expect("posts table");
    // Implicit id (synthesized, primary_key=true) + explicit title.
    assert_eq!(posts.columns.len(), 2);
    let id = &posts.columns[0];
    assert_eq!(id.name.as_str(), "id");
    assert!(id.primary_key);
    assert!(matches!(id.col_type, ColumnType::BigInt));
    let title = &posts.columns[1];
    assert_eq!(title.name.as_str(), "title");
    assert!(matches!(title.col_type, ColumnType::String { .. }));
    assert!(!title.nullable);

    let comments = app
        .schema
        .tables
        .get(&roundhouse::Symbol::from("comments"))
        .expect("comments table");
    // Implicit id + explicit body + explicit post_id.
    assert_eq!(comments.columns.len(), 3);
    assert_eq!(comments.columns[0].name.as_str(), "id");
    let body = &comments.columns[1];
    assert_eq!(body.name.as_str(), "body");
    assert!(matches!(body.col_type, ColumnType::Text));
    let post_id = &comments.columns[2];
    assert_eq!(post_id.name.as_str(), "post_id");
    assert!(matches!(post_id.col_type, ColumnType::BigInt));
}

#[test]
fn ingests_models_with_derived_attributes() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.models.len(), 2);

    let by_name = |n: &str| {
        app.models
            .iter()
            .find(|m| m.name.0.as_str() == n)
            .unwrap_or_else(|| panic!("no model named {n}"))
    };

    let post = by_name("Post");
    assert_eq!(post.table.0.as_str(), "posts");
    assert!(post.attributes.fields.contains_key(&roundhouse::Symbol::from("title")));

    let comment = by_name("Comment");
    assert_eq!(comment.table.0.as_str(), "comments");
    assert!(comment.attributes.fields.contains_key(&roundhouse::Symbol::from("body")));
    assert!(comment.attributes.fields.contains_key(&roundhouse::Symbol::from("post_id")));
}

#[test]
fn ingests_associations_with_convention_defaults() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    assert_eq!(post.associations.len(), 1);
    match &post.associations[0] {
        Association::HasMany { name, target, foreign_key, through, dependent } => {
            assert_eq!(name.as_str(), "comments");
            assert_eq!(target.0.as_str(), "Comment");
            assert_eq!(foreign_key.as_str(), "post_id");
            assert!(through.is_none());
            assert!(matches!(dependent, Dependent::None));
        }
        other => panic!("expected HasMany, got {other:?}"),
    }

    let comment = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Comment")
        .unwrap();
    assert_eq!(comment.associations.len(), 1);
    match &comment.associations[0] {
        Association::BelongsTo { name, target, foreign_key, optional } => {
            assert_eq!(name.as_str(), "post");
            assert_eq!(target.0.as_str(), "Post");
            assert_eq!(foreign_key.as_str(), "post_id");
            assert!(!optional);
        }
        other => panic!("expected BelongsTo, got {other:?}"),
    }
}

#[test]
fn ingests_validations() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    assert_eq!(post.validations.len(), 1);
    let v = &post.validations[0];
    assert_eq!(v.attribute.as_str(), "title");
    assert_eq!(v.rules.len(), 1);
    assert!(matches!(v.rules[0], ValidationRule::Presence));
}

#[test]
fn ingests_callbacks() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    assert_eq!(post.callbacks.len(), 1);
    let cb = &post.callbacks[0];
    assert!(matches!(cb.hook, CallbackHook::BeforeSave));
    assert_eq!(cb.target.as_str(), "normalize_title");
    assert!(cb.condition.is_none());
}

#[test]
fn ingests_posts_controller_with_actions() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.controllers.len(), 1);
    let ctrl = &app.controllers[0];
    assert_eq!(ctrl.name.0.as_str(), "PostsController");
    assert_eq!(
        ctrl.parent.as_ref().unwrap().0.as_str(),
        "ApplicationController"
    );
    assert_eq!(ctrl.actions.len(), 4);
    let names: Vec<_> = ctrl.actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["index", "show", "create", "destroy"]);

    // index body: `@posts = Post.all` — Assign(Ivar(posts), Send(Some(Const(Post)), "all", []))
    let index = &ctrl.actions[0];
    match *index.body.node {
        ExprNode::Assign { ref target, ref value } => {
            match target {
                LValue::Ivar { name } => assert_eq!(name.as_str(), "posts"),
                other => panic!("expected Ivar(posts), got {other:?}"),
            }
            match *value.node {
                ExprNode::Send { recv: Some(ref recv), ref method, ref args, .. } => {
                    assert_eq!(method.as_str(), "all");
                    assert!(args.is_empty());
                    match *recv.node {
                        ExprNode::Const { ref path } => assert_eq!(path[0].as_str(), "Post"),
                        ref other => panic!("expected Const(Post), got {other:?}"),
                    }
                }
                ref other => panic!("expected Send with receiver, got {other:?}"),
            }
        }
        ref other => panic!("expected Assign, got {other:?}"),
    }

    // show body: `@post = Post.find(params[:id])`
    // Expect: Assign(Ivar(post), Send(Const(Post), "find", [Send(Send(None, "params", []), "[]", [Sym(id)])]))
    let show = &ctrl.actions[1];
    match *show.body.node {
        ExprNode::Assign { ref target, ref value } => {
            assert!(matches!(target, LValue::Ivar { name } if name.as_str() == "post"));
            match *value.node {
                ExprNode::Send { recv: Some(_), ref method, ref args, .. } => {
                    assert_eq!(method.as_str(), "find");
                    assert_eq!(args.len(), 1);
                    // The argument is `params[:id]` — Send(Send(None, params, []), [], [:id])
                    match *args[0].node {
                        ExprNode::Send { recv: Some(ref inner_recv), ref method, ref args, .. } => {
                            assert_eq!(method.as_str(), "[]");
                            assert_eq!(args.len(), 1);
                            // inner_recv is the implicit-self `params` call
                            match *inner_recv.node {
                                ExprNode::Send { recv: None, ref method, ref args, .. } => {
                                    assert_eq!(method.as_str(), "params");
                                    assert!(args.is_empty());
                                }
                                ref other => panic!("expected implicit-self params, got {other:?}"),
                            }
                        }
                        ref other => panic!("expected `[]` Send, got {other:?}"),
                    }
                }
                ref other => panic!("expected Send, got {other:?}"),
            }
        }
        ref other => panic!("expected Assign, got {other:?}"),
    }
}

#[test]
fn ingests_routes_file() {
    use roundhouse::RouteSpec;

    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.routes.entries.len(), 4);

    fn as_explicit(spec: &RouteSpec) -> (&HttpMethod, &str, &str, &str, Option<&str>) {
        let RouteSpec::Explicit { method, path, controller, action, as_name, .. } = spec
        else {
            panic!("expected Explicit, got {spec:?}");
        };
        (
            method,
            path.as_str(),
            controller.0.as_str(),
            action.as_str(),
            as_name.as_ref().map(|s| s.as_str()),
        )
    }

    let (m, path, ctrl, action, name) = as_explicit(&app.routes.entries[0]);
    assert!(matches!(m, HttpMethod::Get));
    assert_eq!(path, "/posts");
    assert_eq!(ctrl, "PostsController");
    assert_eq!(action, "index");
    assert_eq!(name, Some("posts"));

    let (m, path, _, action, _) = as_explicit(&app.routes.entries[1]);
    assert!(matches!(m, HttpMethod::Post));
    assert_eq!(path, "/posts");
    assert_eq!(action, "create");

    let (m, path, _, action, name) = as_explicit(&app.routes.entries[2]);
    assert!(matches!(m, HttpMethod::Get));
    assert_eq!(path, "/posts/:id");
    assert_eq!(action, "show");
    assert_eq!(name, Some("post"));

    let (m, path, _, action, _) = as_explicit(&app.routes.entries[3]);
    assert!(matches!(m, HttpMethod::Delete));
    assert_eq!(path, "/posts/:id");
    assert_eq!(action, "destroy");
}

#[test]
fn ingested_app_is_self_consistent() {
    use roundhouse::RouteSpec;

    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.schema_version, roundhouse::App::SCHEMA_VERSION);
    // Serialize / deserialize proves the ingested shape is round-trippable.
    let json = serde_json::to_string_pretty(&app).expect("serialize");
    let _: roundhouse::App = serde_json::from_str(&json).expect("deserialize");
    // Make sure the Rails dependency between route and controller is intact.
    let ctrl_names: Vec<_> = app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    for entry in &app.routes.entries {
        if let RouteSpec::Explicit { controller, .. } = entry {
            assert!(
                ctrl_names.contains(&controller.0.as_str()),
                "route references unknown controller {:?}",
                controller
            );
        }
    }
}

#[test]
fn literal_ingested_expr() {
    let source = br#"42"#;
    let result = ruby_prism::parse(source);
    let program = result.node();
    let prog = program.as_program_node().unwrap();
    let stmt = prog.statements().body().iter().next().unwrap();
    let expr = roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap();
    match *expr.node {
        ExprNode::Lit { value: Literal::Int { value } } => assert_eq!(value, 42),
        ref other => panic!("expected Lit(Int 42), got {other:?}"),
    }
    let _ = Expr::new(expr.span, *expr.node); // just making sure imports are alive
}

#[test]
fn string_interpolation() {
    fn parse_one(source: &[u8]) -> roundhouse::expr::Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    let e = parse_one(br#""article_#{@article.id}_comments""#);
    match &*e.node {
        ExprNode::StringInterp { parts } => {
            assert_eq!(parts.len(), 3);
            match &parts[0] {
                InterpPart::Text { value } => assert_eq!(value.as_str(), "article_"),
                other => panic!("part 0: expected Text, got {other:?}"),
            }
            match &parts[1] {
                InterpPart::Expr { expr } => {
                    // `@article.id` → Send(recv=Ivar(article), method=id)
                    assert!(matches!(&*expr.node, ExprNode::Send { .. }));
                }
                other => panic!("part 1: expected Expr, got {other:?}"),
            }
            match &parts[2] {
                InterpPart::Text { value } => assert_eq!(value.as_str(), "_comments"),
                other => panic!("part 2: expected Text, got {other:?}"),
            }
        }
        other => panic!("expected StringInterp, got {other:?}"),
    }
}

#[test]
fn array_literal_styles() {
    use roundhouse::expr::ArrayStyle;

    fn parse_one(source: &[u8]) -> roundhouse::expr::Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    // Bracket form with symbol elements.
    let e = parse_one(br"[:a, :b, :c]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::Brackets));
            assert_eq!(elements.len(), 3);
            for el in elements {
                assert!(matches!(&*el.node, ExprNode::Lit { value: Literal::Sym { .. } }));
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }

    // %i[ ... ] symbol-list form.
    let e = parse_one(br"%i[show edit update]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::PercentI));
            assert_eq!(elements.len(), 3);
            match &*elements[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    assert_eq!(value.as_str(), "show");
                }
                other => panic!("expected Sym, got {other:?}"),
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }

    // %w[ ... ] word-list form.
    let e = parse_one(br"%w[alpha beta]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::PercentW));
            assert_eq!(elements.len(), 2);
            match &*elements[0].node {
                ExprNode::Lit { value: Literal::Str { value } } => {
                    assert_eq!(value.as_str(), "alpha");
                }
                other => panic!("expected Str, got {other:?}"),
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }
}
