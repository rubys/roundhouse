//! `ingest::app::app_roots` — a Packwerk app's per-package `app/` trees
//! (`packs/<name>/app`, `components/<name>/app`, …) are app-layer roots
//! too, not just the root `app/`. See the doc comment on `app_roots` in
//! `src/ingest/app.rs` for the design; this pins the four load-bearing
//! shapes: default discovery, an explicit `package_paths:` glob, the
//! no-Packwerk case (behavior unchanged), and a deeper default-scan hit.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.string "body"
  end
  create_table "erp_ledger_entries", force: :cascade do |t|
    t.string "description"
  end
end
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const ARTICLE_MODEL: &str = "class Article < ApplicationRecord\nend\n";
const COMMENT_MODEL: &str = "class Comment < ApplicationRecord\nend\n";
const COMMENTS_CONTROLLER: &str =
    "class CommentsController < ApplicationController\n  def index\n    @comments = Comment.all\n  end\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const COMMENTS_INDEX_VIEW: &str = "<%= @comments.length %>\n";
const COMMENT_RANKER: &str = "class CommentRanker\n  def self.rank(comments)\n    comments\n  end\nend\n";

fn tree_app(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

/// (a) `packwerk.yml` with no `package_paths:` (the common shape,
/// where the key is commented out): the default `**/` scan finds
/// `packs/blog/package.yml` and walks its `app/` tree exactly like the
/// root's.
#[test]
fn default_scan_walks_a_packwerk_package() {
    let app = tree_app(&[
        ("packwerk.yml", "# package_paths: **/\n"),
        ("package.yml", "enforce_dependencies: true\n"),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("packs/blog/package.yml", "enforce_dependencies: true\n"),
        ("packs/blog/app/models/comment.rb", COMMENT_MODEL),
        ("packs/blog/app/controllers/comments_controller.rb", COMMENTS_CONTROLLER),
        ("packs/blog/app/views/comments/index.html.erb", COMMENTS_INDEX_VIEW),
        ("packs/blog/app/services/comment_ranker.rb", COMMENT_RANKER),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "packs/blog/app".to_string()]);

    assert!(
        app.models.iter().any(|m| m.name.0.as_str() == "Article"),
        "root app/models/article.rb should still be a model: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );
    assert!(
        app.models.iter().any(|m| m.name.0.as_str() == "Comment"),
        "packs/blog/app/models/comment.rb should be a model: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "CommentsController"),
        "packs/blog/app/controllers/comments_controller.rb should be a controller"
    );
    assert!(
        app.views.iter().any(|v| v.name.as_str() == "comments/index"),
        "packs/blog/app/views/comments/index.html.erb should address as comments/index: {:?}",
        app.views.iter().map(|v| v.name.as_str().to_string()).collect::<Vec<_>>()
    );
    assert!(
        app.library_classes.iter().any(|c| c.name.0.as_str() == "CommentRanker"),
        "packs/blog/app/services/comment_ranker.rb should register CommentRanker: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}

/// (b) An explicit `package_paths: components/*` only walks packages
/// under `components/`; a stray `packs/other/package.yml` with its own
/// `app/` is not a Packwerk package by this app's config and must not
/// be walked.
#[test]
fn explicit_package_paths_glob_scopes_discovery() {
    let app = tree_app(&[
        ("packwerk.yml", "package_paths: components/*\n"),
        ("package.yml", "enforce_dependencies: true\n"),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("components/blog/package.yml", "enforce_dependencies: true\n"),
        ("components/blog/app/models/comment.rb", COMMENT_MODEL),
        // Not matched by `components/*` — must not become a root even
        // though it has its own package.yml and app/ tree.
        ("packs/other/package.yml", "enforce_dependencies: true\n"),
        ("packs/other/app/models/stray.rb", "class Stray < ApplicationRecord\nend\n"),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "components/blog/app".to_string()]);
    assert!(app.models.iter().any(|m| m.name.0.as_str() == "Comment"));
    assert!(
        !app.models.iter().any(|m| m.name.0.as_str() == "Stray"),
        "packs/other is outside package_paths and must not be walked: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn brace_package_paths_select_nested_packages_and_exclude_unmatched_packages() {
    let app = tree_app(&[
        ("packwerk.yml", "package_paths: \"{,components,components/*/,components/*/*/}\"\n"),
        ("package.yml", "enforce_dependencies: true\n"),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("components/inventory/package.yml", "enforce_dependencies: true\n"),
        ("components/inventory/app/models/comment.rb", COMMENT_MODEL),
        ("components/domains/shop/package.yml", "enforce_dependencies: true\n"),
        ("components/domains/shop/app/models/erp_ledger_entry.rb", "class ErpLedgerEntry < ApplicationRecord\nend\n"),
        ("packs/unused/package.yml", "enforce_dependencies: true\n"),
        ("packs/unused/app/models/stray.rb", "class Stray < ApplicationRecord\nend\n"),
    ]);
    assert_eq!(app.app_roots, vec![
        "app", "components/domains/shop/app", "components/inventory/app",
    ]);
    assert!(app.models.iter().any(|model| model.name.0.as_str() == "Comment"));
    assert!(app.models.iter().any(|model| model.name.0.as_str() == "ErpLedgerEntry"));
    assert!(!app.models.iter().any(|model| model.name.0.as_str() == "Stray"));
}

/// (c) No `packwerk.yml` / `packs.yml` at all: a `packs/blog/app/…`
/// tree is ordinary non-autoloaded data, not a second app root — the
/// exact behavior an app without Packwerk had before this change.
#[test]
fn no_packwerk_config_leaves_behavior_unchanged() {
    let app = tree_app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("packs/blog/package.yml", "enforce_dependencies: true\n"),
        ("packs/blog/app/models/comment.rb", COMMENT_MODEL),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string()]);
    assert!(app.models.iter().any(|m| m.name.0.as_str() == "Article"));
    assert!(
        !app.models.iter().any(|m| m.name.0.as_str() == "Comment"),
        "without packwerk.yml/packs.yml, packs/blog/app must not be walked: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );
}

/// (d) The default `**/` scan finds a package two levels deep
/// (`engines/erp/package.yml`), matching the Procore monolith's
/// `engines/erp` shape.
#[test]
fn default_scan_finds_a_package_two_levels_deep() {
    let app = tree_app(&[
        ("packwerk.yml", "# package_paths: **/\n"),
        ("package.yml", "enforce_dependencies: true\n"),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("engines/erp/package.yml", "enforce_dependencies: true\n"),
        ("engines/erp/app/models/erp_ledger_entry.rb", "class ErpLedgerEntry < ApplicationRecord\nend\n"),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "engines/erp/app".to_string()]);
    assert!(
        app.models.iter().any(|m| m.name.0.as_str() == "ErpLedgerEntry"),
        "engines/erp/app/models/erp_ledger_entry.rb should be discovered by the default scan: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );
}
