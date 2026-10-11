//! friendly_id typing (`lower::friendly_id`, `analyze::register_friendly_id`):
//! `Model.friendly` is the model's Relation, so the EngineeredAt
//! `before_action` finder `@article = Article.friendly.find(params[:id])`
//! resolves `@article` instead of failing dispatch on `friendly` and
//! cascading `ivar_unresolved` into every view and action that reads it.
//! Execution of the finder is pinned in `tests/emit_and_run.rs`
//! (`friendly_find_*`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

const SCHEMA_RB: &str = r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.string "slug"
  end
end
"#;

const CONTROLLER: &str = r#"class ArticlesController < ApplicationController
  before_action :set_article, only: :show
  before_action :set_other, only: :index

  def show
    render plain: @article.title
  end

  def index
    render plain: @other.title
  end

  private

  def set_article
    @article = Article.friendly.find(params[:id])
  end

  def set_other
    @other = Article.where(title: "x").friendly.find(params[:id])
  end
end
"#;

fn analyze(model: &str) -> roundhouse::App {
    analyze_with(model, CONTROLLER)
}

fn analyze_with(model: &str, controller: &str) -> roundhouse::App {
    let files: [(&str, &str); 4] = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/article.rb", model),
        ("app/controllers/articles_controller.rb", controller),
        ("db/schema.rb", SCHEMA_RB),
    ];
    let mut tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        b"class ApplicationController < ActionController::Base\nend\n".to_vec(),
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn kinds(app: &roundhouse::App) -> (Vec<String>, Vec<String>) {
    let mut dispatch = Vec::new();
    let mut ivars = Vec::new();
    for d in diagnose(app) {
        match d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => dispatch.push(method.as_str().to_string()),
            DiagnosticKind::IvarUnresolved { name } => ivars.push(name.as_str().to_string()),
            _ => {}
        }
    }
    (dispatch, ivars)
}

#[test]
fn friendly_is_the_models_relation_and_the_filter_ivar_resolves() {
    let app = analyze(
        "class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: %i[slugged history]\nend\n",
    );
    let (dispatch, ivars) = kinds(&app);
    assert!(
        !dispatch.iter().any(|m| m == "friendly"),
        "friendly must dispatch on Article and on Relation[Article]; failures = {dispatch:?}"
    );
    assert!(
        !ivars.iter().any(|n| n == "article" || n == "other"),
        "@article / @other should resolve behind `friendly.find`; unresolved = {ivars:?}"
    );
}

/// A configuration the finder lowering cannot honor is declined whole:
/// `:scoped` changes what a slug means, and `:finders` replaces
/// `Model.find` itself. The model keeps its unsupported warnings and
/// `friendly` stays an unknown method, so the error stands instead of a
/// finder with different semantics being emitted.
#[test]
fn an_unsupported_friendly_id_configuration_keeps_the_error() {
    for modules in ["%i[slugged scoped]", "%i[slugged finders]", "%i[history]", ":simple_i18n"] {
        let model = format!(
            "class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: {modules}\nend\n"
        );
        let app = analyze(&model);
        let (dispatch, _) = kinds(&app);
        assert!(
            dispatch.iter().any(|m| m == "friendly"),
            "`use: {modules}` must not be claimed; dispatch failures = {dispatch:?}"
        );
    }
}

/// `slug_column:` is honored, and a model without the slug column is
/// declined (the finder would query a column that is not there).
#[test]
fn the_slug_column_must_exist() {
    let ok = analyze(
        "class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: :slugged, slug_column: :title\nend\n",
    );
    assert!(!kinds(&ok).0.iter().any(|m| m == "friendly"));
    let missing = analyze(
        "class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: :slugged, slug_column: :permalink\nend\n",
    );
    assert!(kinds(&missing).0.iter().any(|m| m == "friendly"));
}

/// `friendly` on a RELATION is refused for a model that did not claim
/// friendly_id, so the only compile-clean `friendly` sites are ones the
/// runtime answers (it raises for an unclaimed model).
#[test]
fn friendly_on_a_relation_of_an_unclaimed_model_is_refused() {
    let controller = r#"class ArticlesController < ApplicationController
  before_action :set_other, only: :index

  def index
    render plain: @other.title
  end

  private

  def set_other
    @other = Article.where(title: "x").friendly.find(params[:id])
  end
end
"#;
    let unclaimed = analyze_with("class Article < ApplicationRecord\nend\n", controller);
    assert!(kinds(&unclaimed).0.iter().any(|m| m == "friendly"));
    let claimed = analyze_with(
        "class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: :slugged\nend\n",
        controller,
    );
    assert!(!kinds(&claimed).0.iter().any(|m| m == "friendly"));
    // A relation held in a local (Relation-typed, not the inline chain).
    let held = controller.replace(
        "Article.where(title: \"x\").friendly.find",
        "Article.order(:title).limit(3).friendly.find",
    );
    let unclaimed = analyze_with("class Article < ApplicationRecord\nend\n", &held);
    assert!(kinds(&unclaimed).0.iter().any(|m| m == "friendly"));
}
