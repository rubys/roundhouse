//! A missing or malformed strong-params resource refuses the request
//! the way Rails does, on the ruby family: the typed factory is handed
//! `Params.expect_present(@params, …)` or `Params.require_present(@params,
//! …)`, by the source form. The strict targets have no exception control
//! flow and hand-written `Params` primitives, so their factory call keeps
//! `@params` (ledgered in docs/pipeline/runtime.md). Behavior against
//! Rails is pinned in `emit_and_run.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n    t.text :body\n  end\nend\n";

fn controller_for(target: BuildTarget, helper_body: &str) -> String {
    let controller = format!(
        "class ArticlesController < ApplicationController\n  def create\n    @article = Article.new(article_params)\n    @article.save\n    head :created\n  end\n\n  private\n\n  def article_params\n    {helper_body}\n  end\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :articles, only: :create\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/articles_controller.rb"), controller.into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    files
        .into_iter()
        .filter(|(p, _)| p.contains("articles_controller"))
        .map(|(_, c)| c)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn expect_is_guarded_by_its_own_refusal_on_the_ruby_family() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = controller_for(target, "params.expect(article: [ :title, :body ])");
        assert!(
            src.contains(r#"ArticleParams.from_raw(Params.expect_present(@params, "article", ["title", "body"], [], [], []))"#),
            "{target:?}:\n{src}"
        );
    }
}

#[test]
fn require_permit_is_guarded_by_the_laxer_refusal() {
    let src = controller_for(BuildTarget::Ruby, "params.require(:article).permit(:title, :body)");
    assert!(
        src.contains(r#"ArticleParams.from_raw(Params.require_present(@params, "article"))"#),
        "{src}"
    );
}

#[test]
fn strict_targets_keep_the_unguarded_factory() {
    // Crystal stands in for every target outside the ruby family.
    let src = controller_for(BuildTarget::Crystal, "params.expect(article: [ :title, :body ])");
    assert!(!src.contains("expect_present"), "{src}");
    assert!(!src.contains("require_present"), "{src}");
    assert!(src.contains("from_raw"), "{src}");
}

/// Each nested key of an `expect` filter is passed by the kind of value
/// its filter takes, as Rails' `expect` checks it: `tags: []` an array of
/// scalars, `settings: [:theme]` and `prefs: {}` a hash, `items: [[:name]]`
/// an array of hashes.
#[test]
fn expect_passes_nested_keys_by_the_value_their_filter_takes() {
    let src = controller_for(
        BuildTarget::Ruby,
        "params.expect(article: [ :title, settings: [ :theme ], tags: [], items: [ [ :name ] ], prefs: {} ])",
    );
    assert!(
        src.contains(r#"Params.expect_present(@params, "article", ["title"], ["tags"], ["settings", "prefs"], ["items"])"#),
        "{src}"
    );
}
