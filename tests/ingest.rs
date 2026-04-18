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
    let post_assocs: Vec<&Association> = post.associations().collect();
    assert_eq!(post_assocs.len(), 1);
    match post_assocs[0] {
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
    let comment_assocs: Vec<&Association> = comment.associations().collect();
    assert_eq!(comment_assocs.len(), 1);
    match comment_assocs[0] {
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
    let validations: Vec<&roundhouse::Validation> = post.validations().collect();
    assert_eq!(validations.len(), 1);
    let v = validations[0];
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
    let callbacks: Vec<&roundhouse::Callback> = post.callbacks().collect();
    assert_eq!(callbacks.len(), 1);
    let cb = callbacks[0];
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
    let actions: Vec<&roundhouse::Action> = ctrl.actions().collect();
    assert_eq!(actions.len(), 4);
    let names: Vec<_> = actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["index", "show", "create", "destroy"]);

    // index body: `@posts = Post.all` — Assign(Ivar(posts), Send(Some(Const(Post)), "all", []))
    let index = actions[0];
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
    let show = actions[1];
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
fn block_delimiter_style_is_preserved() {
    use roundhouse::{BlockStyle, ExprNode, ModelBodyItem};

    // Two consecutive class-body calls with different block delimiters —
    // the Ruby emitter uses the preserved style to pick `{ }` vs do…end.
    let source = br#"
class Widget < ApplicationRecord
  after_create_commit { ping }
  after_destroy_commit do
    pong
  end
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 2);

    fn block_style_of(item: &ModelBodyItem) -> BlockStyle {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            panic!("expected Unknown body item, got {item:?}");
        };
        let ExprNode::Send { block: Some(block), .. } = &*expr.node else {
            panic!("expected Send-with-block");
        };
        let ExprNode::Lambda { block_style, .. } = &*block.node else {
            panic!("expected Lambda block");
        };
        *block_style
    }

    assert!(matches!(block_style_of(&model.body[0]), BlockStyle::Brace));
    assert!(matches!(block_style_of(&model.body[1]), BlockStyle::Do));
}

#[test]
fn leading_comments_attach_to_class_body_items() {
    use roundhouse::ModelBodyItem;

    let source = br#"
class Widget < ApplicationRecord
  # This comment should attach to has_many below.
  has_many :gears

  # Two comment lines
  # both attach to validates
  validates :name, presence: true
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 2);

    let ModelBodyItem::Association { leading_comments: has_many_comments, .. } = &model.body[0]
    else {
        panic!("expected Association first");
    };
    assert_eq!(has_many_comments.len(), 1);
    assert_eq!(
        has_many_comments[0].text.as_str(),
        "# This comment should attach to has_many below."
    );

    let ModelBodyItem::Validation { leading_comments: validates_comments, .. } = &model.body[1]
    else {
        panic!("expected Validation second");
    };
    assert_eq!(validates_comments.len(), 2);
    assert_eq!(validates_comments[0].text.as_str(), "# Two comment lines");
    assert_eq!(
        validates_comments[1].text.as_str(),
        "# both attach to validates"
    );
}

#[test]
fn length_validation_rule_is_ingested() {
    use roundhouse::{ModelBodyItem, ValidationRule};

    let source = br#"
class Widget < ApplicationRecord
  validates :body, presence: true, length: { minimum: 10 }
  validates :title, length: { maximum: 80 }
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
        .unwrap()
        .unwrap();

    let validations: Vec<_> = model.body.iter().filter_map(|item| match item {
        ModelBodyItem::Validation { validation, .. } => Some(validation),
        _ => None,
    }).collect();

    // `validates :body, presence: true, length: { minimum: 10 }` expands
    // to one Validation for :body with two rules.
    assert_eq!(validations.len(), 2);
    let body_v = validations.iter().find(|v| v.attribute.as_str() == "body").unwrap();
    assert_eq!(body_v.rules.len(), 2);
    assert!(body_v.rules.iter().any(|r| matches!(r, ValidationRule::Presence)));
    assert!(body_v.rules.iter().any(
        |r| matches!(r, ValidationRule::Length { min: Some(10), max: None })
    ));

    let title_v = validations.iter().find(|v| v.attribute.as_str() == "title").unwrap();
    assert_eq!(title_v.rules.len(), 1);
    assert!(matches!(
        title_v.rules[0],
        ValidationRule::Length { min: None, max: Some(80) }
    ));
}

#[test]
fn model_body_preserves_source_order_with_unknown_fallback() {
    use roundhouse::ModelBodyItem;

    // Exercise the ingest: a model with a known association, an unknown
    // class-body call (`broadcasts_to`), and a validation — in that
    // order. The body Vec must mirror that exact order, with
    // broadcasts_to captured as `Unknown` (preserved as an Expr rather
    // than silently dropped).
    let source = br#"
class Widget < ApplicationRecord
  has_many :gears
  broadcasts_to :widgets
  validates :name, presence: true
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 3);
    assert!(matches!(model.body[0], ModelBodyItem::Association { .. }));
    match &model.body[1] {
        ModelBodyItem::Unknown { expr, .. } => {
            // `broadcasts_to :widgets` → Send with no receiver, method
            // "broadcasts_to", one symbol arg.
            let ExprNode::Send { method, .. } = &*expr.node else {
                panic!("expected Send, got {:?}", expr.node);
            };
            assert_eq!(method.as_str(), "broadcasts_to");
        }
        other => panic!("expected Unknown(broadcasts_to), got {other:?}"),
    }
    assert!(matches!(model.body[2], ModelBodyItem::Validation { .. }));
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
