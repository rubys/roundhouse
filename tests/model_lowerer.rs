//! Step 3 — first session: model lowerers from Rails-shape `Model` to
//! the universal post-lowering `LibraryClass` whose body is a flat
//! sequence of `MethodDef`s. The forcing function is the spinel-blog
//! fixture pair: real-blog/app/models/article.rb (Rails DSL) lowers to
//! a LibraryClass structurally matching spinel-blog/app/models/article.rb
//! (explicit method bodies).
//!
//! Comparison is structural at the IR level — method names, parameter
//! lists, receiver kinds. Body shapes are spot-checked rather than
//! deep-compared because the spinel-blog fixture is hand-written and
//! carries stylistic choices (variable naming, formatting) that the
//! lowerer's output won't match byte-for-byte. See the handoff for the
//! "structural compare passes ≠ textual match required" calibration.

use std::path::Path;

use roundhouse::dialect::{LibraryClass, MethodReceiver};
use roundhouse::ingest::ingest_app;
use roundhouse::lower::lower_model_to_library_class;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/real-blog")
}

fn lower(name: &str) -> LibraryClass {
    let app = ingest_app(fixture_path()).expect("ingest real-blog");
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == name)
        .unwrap_or_else(|| panic!("model {name} not in real-blog"));
    lower_model_to_library_class(model, &app.schema)
}

fn method_names(lc: &LibraryClass) -> Vec<&str> {
    lc.methods.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn application_record_lowers_to_empty_library_class() {
    // application_record.rb is abstract — no schema table, no
    // associations, no validations. Lowering should produce a
    // LibraryClass with the right name + parent + zero methods.
    let lc = lower("ApplicationRecord");
    assert_eq!(lc.name.0.as_str(), "ApplicationRecord");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ActiveRecord::Base", "parent: {parent}");
    assert!(!lc.is_module);
    assert!(
        lc.methods.is_empty(),
        "ApplicationRecord lowering should have no methods (got {:?})",
        method_names(&lc),
    );
}

#[test]
fn article_lowers_with_schema_methods() {
    let lc = lower("Article");
    assert_eq!(lc.name.0.as_str(), "Article");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ApplicationRecord");

    let names = method_names(&lc);

    // Per-column accessors (excluding id — inherits from base).
    for col in ["title", "body", "created_at", "updated_at"] {
        assert!(names.contains(&col), "missing reader `{col}`: {names:?}");
        let writer = format!("{col}=");
        assert!(
            names.iter().any(|n| *n == writer.as_str()),
            "missing writer `{writer}`: {names:?}",
        );
    }
    // No id reader / writer (id comes from ActiveRecord::Base).
    assert!(
        !names.contains(&"id"),
        "id reader should not be synthesized; methods: {names:?}",
    );

    // The non-attr scaffold: table_name, schema_columns, instantiate,
    // initialize, attributes, [], []=, update.
    for expected in [
        "table_name",
        "schema_columns",
        "instantiate",
        "initialize",
        "attributes",
        "[]",
        "[]=",
        "update",
    ] {
        assert!(
            names.contains(&expected),
            "missing scaffold method `{expected}`: {names:?}",
        );
    }

    // Receiver checks: table_name, schema_columns, instantiate are class
    // methods; everything else is instance.
    let class_methods = ["table_name", "schema_columns", "instantiate"];
    for m in &lc.methods {
        let n = m.name.as_str();
        if class_methods.contains(&n) {
            assert!(
                matches!(m.receiver, MethodReceiver::Class),
                "`{n}` should be a class method, got {:?}",
                m.receiver,
            );
        } else {
            assert!(
                matches!(m.receiver, MethodReceiver::Instance),
                "`{n}` should be an instance method, got {:?}",
                m.receiver,
            );
        }
    }
}

#[test]
fn article_lowers_has_many_to_collection_reader() {
    let lc = lower("Article");
    let comments = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "comments")
        .expect("comments method present (has_many :comments)");

    assert!(matches!(comments.receiver, MethodReceiver::Instance));
    assert!(comments.params.is_empty());

    // Body should be `Comment.where(article_id: @id)`.
    let (recv_path, method) = match &*comments.body.node {
        roundhouse::ExprNode::Send { recv, method, .. } => {
            let recv = recv.as_ref().expect("comments body should be Comment.where(...)");
            let path = match &*recv.node {
                roundhouse::ExprNode::Const { path } => {
                    path.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>()
                }
                other => panic!("comments receiver should be Const; got {other:?}"),
            };
            (path, method.as_str().to_string())
        }
        other => panic!("comments body is not Send: {other:?}"),
    };
    assert_eq!(recv_path, vec!["Comment".to_string()]);
    assert_eq!(method, "where");
}

#[test]
fn comment_lowers_belongs_to_reader() {
    let lc = lower("Comment");
    assert_eq!(lc.name.0.as_str(), "Comment");

    let article = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "article")
        .expect("article method present (belongs_to :article)");

    assert!(matches!(article.receiver, MethodReceiver::Instance));
    assert!(article.params.is_empty());

    // Shape: `if @article_id == 0 then nil else Article.find_by(id: @article_id) end`.
    match &*article.body.node {
        roundhouse::ExprNode::If { cond, .. } => match &*cond.node {
            roundhouse::ExprNode::Send { method, .. } => {
                assert_eq!(method.as_str(), "==", "guard should be ==");
            }
            other => panic!("if-cond should be Send `==`; got {other:?}"),
        },
        other => panic!("article body should be If; got {other:?}"),
    }
}

#[test]
fn article_lowers_dependent_destroy_to_before_destroy() {
    let lc = lower("Article");
    let cb = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "before_destroy")
        .expect("before_destroy method present (has_many dependent: :destroy)");

    assert!(matches!(cb.receiver, MethodReceiver::Instance));
    let body = &*cb.body.node;
    let exprs = match body {
        roundhouse::ExprNode::Seq { exprs } => exprs.clone(),
        // Single statement collapses to non-Seq; treat as one-element list.
        _ => vec![cb.body.clone()],
    };
    assert!(!exprs.is_empty(), "before_destroy should not be empty");
    // First (and only) statement: `comments.each { |c| c.destroy }`.
    let first = &exprs[0];
    let (method, block_present) = match &*first.node {
        roundhouse::ExprNode::Send { method, block, .. } => (method.as_str(), block.is_some()),
        other => panic!("expected each-Send in before_destroy; got {other:?}"),
    };
    assert_eq!(method, "each");
    assert!(block_present, "each call should carry a block");
}
