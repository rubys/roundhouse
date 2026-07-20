//! Ingest-level checks for the Roda + Sequel front-end (issue #67)
//! against `fixtures/roda-blog`. These assert the shape of the
//! ingested `App` — routes linearized from the routing tree,
//! controllers synthesized with prologue filters, Sequel models and
//! migrations folded into the shared IR — per the mapping table in
//! `docs/roda-sequel-plan.md`.

use roundhouse::dialect::{
    Association, ControllerBodyItem, Dependent, FilterKind, LayoutDecl,
    RouteSpec, ValidationRule,
};
use roundhouse::ingest::ingest_app;
use roundhouse::schema::{ColumnType, ReferentialAction};
use std::path::Path;

fn ingest() -> roundhouse::App {
    ingest_app(Path::new("fixtures/roda-blog")).expect("roda-blog ingests")
}

#[test]
fn ingests_without_parse_diagnostics() {
    let (result, diags) =
        roundhouse::ingest::prism::scope(|| ingest_app(Path::new("fixtures/roda-blog")));
    result.expect("ingest ok");
    assert!(diags.is_empty(), "parse diagnostics: {diags:?}");
}

#[test]
fn schema_folds_from_sequel_migrations() {
    let app = ingest();
    assert_eq!(app.schema.tables.len(), 2);

    let articles = &app.schema.tables[&roundhouse::Symbol::from("articles")];
    let id = articles.columns.iter().find(|c| c.name.as_str() == "id").unwrap();
    assert!(id.primary_key);
    let title = articles.columns.iter().find(|c| c.name.as_str() == "title").unwrap();
    assert_eq!(title.col_type, ColumnType::String { limit: None });
    assert!(!title.nullable, "null: false carried");
    let body = articles.columns.iter().find(|c| c.name.as_str() == "body").unwrap();
    assert_eq!(body.col_type, ColumnType::Text, "String :body, text: true → Text");

    let comments = &app.schema.tables[&roundhouse::Symbol::from("comments")];
    let fk = comments.foreign_keys.first().expect("comments has an FK");
    assert_eq!(fk.from_column.as_str(), "article_id");
    assert_eq!(fk.to_table.0.as_str(), "articles");
    assert_eq!(fk.on_delete, ReferentialAction::Cascade);
    assert_eq!(comments.indexes.len(), 1);
    let article_id =
        comments.columns.iter().find(|c| c.name.as_str() == "article_id").unwrap();
    assert!(matches!(&article_id.col_type, ColumnType::Reference { table } if table.0.as_str() == "articles"));
}

#[test]
fn sequel_models_converge_on_rails_shape() {
    let app = ingest();
    // Article + Comment + the synthesized ApplicationRecord base.
    assert_eq!(app.models.len(), 3);

    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(article.parent.as_ref().unwrap().0.as_str(), "ApplicationRecord");
    assert_eq!(article.table.0.as_str(), "articles");
    assert!(!article.attributes.fields.is_empty(), "attributes folded from schema");
    let has_many = article.associations().next().expect("one_to_many ingested");
    match has_many {
        Association::HasMany { name, target, foreign_key, dependent, scope, .. } => {
            assert_eq!(name.as_str(), "comments");
            assert_eq!(target.0.as_str(), "Comment");
            assert_eq!(foreign_key.as_str(), "article_id");
            // FK ON DELETE CASCADE in the migration expresses the same
            // behavior dependent: :destroy does on the AR runtime.
            assert_eq!(*dependent, Dependent::Destroy);
            assert!(scope.is_some(), "order: Sequel.desc(...) carried as assoc scope");
        }
        other => panic!("expected HasMany, got {other:?}"),
    }
    // `def validate` body → declarative Validations: presence on
    // title+body, min-length 10 with the custom message on body.
    let validations: Vec<_> = article.validations().collect();
    assert!(
        validations
            .iter()
            .any(|v| v.attribute.as_str() == "title"
                && v.rules.iter().any(|r| matches!(r, ValidationRule::Presence))),
        "presence on title: {validations:?}"
    );
    assert!(
        validations.iter().any(|v| v.attribute.as_str() == "body"
            && v.rules.iter().any(|r| matches!(
                r,
                ValidationRule::Length { min: Some(10), max: None, message: Some(m) }
                    if m == "must be at least 10 characters"
            ))),
        "min-length with custom message on body: {validations:?}"
    );

    let comment = app.models.iter().find(|m| m.name.0.as_str() == "Comment").unwrap();
    let belongs_to = comment.associations().next().expect("many_to_one ingested");
    match belongs_to {
        Association::BelongsTo { name, target, foreign_key, optional, .. } => {
            assert_eq!(name.as_str(), "article");
            assert_eq!(target.0.as_str(), "Article");
            assert_eq!(foreign_key.as_str(), "article_id");
            assert!(!optional, "article_id is NOT NULL → required");
        }
        other => panic!("expected BelongsTo, got {other:?}"),
    }
}

#[test]
fn routing_tree_linearizes_to_rest_routes() {
    let app = ingest();
    let mut flat: Vec<(String, String, String, String)> = Vec::new();
    for entry in &app.routes.entries {
        match entry {
            RouteSpec::Explicit { method, path, controller, action, .. } => flat.push((
                format!("{method:?}"),
                path.clone(),
                controller.0.to_string(),
                action.to_string(),
            )),
            RouteSpec::Root { target } => {
                flat.push(("Get".into(), "/".into(), "root".into(), target.clone()))
            }
            other => panic!("unexpected route spec: {other:?}"),
        }
    }
    let expect = [
        ("Get", "/", "root", "root#index"),
        ("Get", "/articles", "ArticlesController", "index"),
        ("Post", "/articles", "ArticlesController", "create"),
        ("Get", "/articles/new", "ArticlesController", "new"),
        ("Get", "/articles/:id", "ArticlesController", "show"),
        ("Patch", "/articles/:id", "ArticlesController", "update"),
        ("Delete", "/articles/:id", "ArticlesController", "destroy"),
        ("Get", "/articles/:id/edit", "ArticlesController", "edit"),
        ("Post", "/articles/:id/comments", "CommentsController", "create"),
        (
            "Delete",
            "/articles/:id/comments/:comment_id",
            "CommentsController",
            "destroy",
        ),
    ];
    let flat_refs: Vec<(&str, &str, &str, &str)> = flat
        .iter()
        .map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str()))
        .collect();
    for want in &expect {
        assert!(flat_refs.contains(want), "missing route {want:?} in {flat:?}");
    }
    assert_eq!(flat.len(), expect.len(), "no extra routes: {flat:?}");
}

#[test]
fn controllers_synthesize_with_prologue_filters() {
    let app = ingest();
    let names: Vec<&str> = app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    assert_eq!(
        names,
        vec!["ApplicationController", "RootController", "ArticlesController", "CommentsController"]
    );

    let application =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ApplicationController").unwrap();
    // The one app-wide layout re-homes onto layouts/application (the
    // convention default), so no explicit declaration is needed.
    assert!(matches!(&application.layout, LayoutDecl::Inherit));

    let articles =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ArticlesController").unwrap();
    let actions: Vec<&str> = articles.actions().map(|a| a.name.as_str()).collect();
    // Route-leaf actions in source order, plus the synthesized
    // prologue method after the private marker.
    assert_eq!(
        actions,
        vec![
            "index",
            "create",
            "new",
            "show",
            "update",
            "destroy",
            "edit",
            "set_article",
            "article_params"
        ]
    );
    let filter = articles.filters().next().expect("prologue filter");
    assert_eq!(filter.kind, FilterKind::Before);
    assert_eq!(filter.target.as_str(), "set_article");
    let mut only: Vec<&str> = filter.only.iter().map(|s| s.as_str()).collect();
    only.sort_unstable();
    assert_eq!(only, vec!["destroy", "edit", "show", "update"]);
    assert!(
        articles
            .body
            .iter()
            .any(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. })),
        "private marker before the synthesized filter method"
    );

    let comments =
        app.controllers.iter().find(|c| c.name.0.as_str() == "CommentsController").unwrap();
    let actions: Vec<&str> = comments.actions().map(|a| a.name.as_str()).collect();
    assert_eq!(actions, vec!["create", "destroy", "set_article", "comment_params"]);
    let filter = comments.filters().next().expect("comments share the article prologue");
    assert_eq!(filter.target.as_str(), "set_article");
    assert!(filter.only.is_empty(), "guards every comments action");
}

#[test]
fn bodies_normalize_to_ar_vocabulary() {
    let app = ingest();
    // No Sequel/Roda surface survives in any controller body: the
    // rewrites in ingest normalize to the AR + Rails-controller
    // vocabulary the rest of the pipeline speaks.
    let banned = ["set_fields", "with_pk", "eager", "comments_dataset", "view"];
    let mut sends: Vec<String> = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            collect_sends(&action.body, &mut sends);
        }
    }
    for method in banned {
        assert!(
            !sends.iter().any(|s| s == method),
            "{method} survived normalization; sends: {sends:?}"
        );
    }
    // And the normalized spellings are present.
    for expected in ["find_by", "includes", "expect", "update", "redirect_to", "render"] {
        assert!(
            sends.iter().any(|s| s == expected),
            "expected a {expected} call after normalization; sends: {sends:?}"
        );
    }

    // The Integer matcher's block param reads as params[:id], so the
    // show/update/destroy prologue is Rails-shaped.
    let articles =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ArticlesController").unwrap();
    let set_article = articles.actions().find(|a| a.name.as_str() == "set_article").unwrap();
    let mut prologue_sends = Vec::new();
    collect_sends(&set_article.body, &mut prologue_sends);
    assert!(prologue_sends.iter().any(|s| s == "params"), "prologue reads params");
    assert!(prologue_sends.iter().any(|s| s == "find_by"), "Article[id] → find_by");
    assert!(prologue_sends.iter().any(|s| s == "render"), "missing record renders 404");
}

#[test]
fn views_ingest_with_roda_dialect_normalized() {
    let app = ingest();
    let names: Vec<&str> = app.views.iter().map(|v| v.name.as_str()).collect();
    for expected in [
        "layouts/application",
        "not_found",
        "articles/index",
        "articles/show",
        "articles/new",
        "articles/edit",
        "articles/_form",
        "articles/_article",
        "comments/_comment",
    ] {
        assert!(names.contains(&expected), "missing view {expected} in {names:?}");
    }

    // `part(...)` normalized to `render(...)` everywhere.
    let mut sends = Vec::new();
    for view in &app.views {
        collect_sends(&view.body, &mut sends);
    }
    assert!(!sends.iter().any(|s| s == "part"), "part survived in views");
    assert!(sends.iter().any(|s| s == "render"), "partials render");
}

#[test]
fn helpers_register_in_the_bare_call_index() {
    let app = ingest();
    for helper in ["truncate", "pluralize"] {
        let owner = app
            .helper_method_index
            .get(&roundhouse::Symbol::from(helper))
            .unwrap_or_else(|| panic!("{helper} not registered"));
        assert_eq!(owner.0.as_str(), "ApplicationHelper");
    }
    assert!(
        app.library_classes.iter().any(|c| c.name.0.as_str() == "ApplicationHelper"),
        "helper module registered as a library class"
    );
}

#[test]
fn seeds_ingest_normalized() {
    let app = ingest();
    let seeds = app.seeds.as_ref().expect("seeds.rb ingested");
    let mut sends = Vec::new();
    collect_sends(seeds, &mut sends);
    assert!(!sends.iter().any(|s| s == "require_relative"), "boot requires stripped");
    assert!(!sends.iter().any(|s| s == "dataset"), "X.dataset.delete → delete_all");
    assert!(sends.iter().any(|s| s == "delete_all"));
    assert!(
        !sends.iter().any(|s| s == "add_comment"),
        "add_comment → comments.create"
    );
    assert!(sends.iter().any(|s| s == "create"));
}

/// The in-browser playground ingests through `ingest_app_from_tree`
/// (an in-memory path→bytes map — the shape `bundle-src.mjs` ships).
/// Guard that the roda front-end dispatch works on that path with the
/// bundle's file subset: analyzable sources only, no Gemfile, no
/// test/, no README.
#[test]
fn ingests_from_in_memory_tree() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    let fixture = Path::new("fixtures/roda-blog");
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for rel in [
        "app.rb",
        "config.ru",
        "db.rb",
        "seeds.rb",
        "db/migrate/001_create_articles.rb",
        "db/migrate/002_create_comments.rb",
        "models/article.rb",
        "models/comment.rb",
        "views/articles/_article.erb",
        "views/articles/_form.erb",
        "views/articles/edit.erb",
        "views/articles/index.erb",
        "views/articles/new.erb",
        "views/articles/show.erb",
        "views/comments/_comment.erb",
        "views/layout.erb",
        "views/not_found.erb",
    ] {
        tree.insert(
            PathBuf::from(rel),
            std::fs::read(fixture.join(rel)).unwrap_or_else(|_| panic!("read {rel}")),
        );
    }
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("tree ingest");
    assert_eq!(app.routes.entries.len(), 10, "routes linearized from the tree");
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "ArticlesController"),
        "controllers synthesized from the tree"
    );
    assert_eq!(app.schema.tables.len(), 2, "migrations folded from the tree");
}

fn collect_sends(expr: &roundhouse::expr::Expr, out: &mut Vec<String>) {
    if let roundhouse::expr::ExprNode::Send { method, .. } = &*expr.node {
        out.push(method.to_string());
    }
    expr.node.for_each_child(&mut |c| collect_sends(c, out));
}
