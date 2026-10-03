//! `roundhouse check` at the process boundary: the exit code is the
//! contract a CI gate reads, so the cases that must NOT report clean
//! are pinned here.

use std::process::Command;

fn check(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .args(args)
        .output()
        .expect("spawn roundhouse");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_missing_path_is_not_a_clean_app() {
    let (code, err) = check(&["/nonexistent/rails/app"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not a directory"), "{err}");
    assert!(!err.contains("0 error(s)"), "must not print a summary: {err}");
}

#[test]
fn a_directory_without_app_is_not_a_clean_app() {
    // The repo root: a directory, but not a Rails app.
    let (code, err) = check(&[env!("CARGO_MANIFEST_DIR")]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("does not look like a Rails app"), "{err}");
}

#[test]
fn malformed_test_path_configuration_is_not_a_clean_app() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse_bad_test_paths_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("app")).expect("create app directory");
    std::fs::write(root.join("roundhouse.yml"), "test_paths: test/unit\n").expect("write config");
    let app = root.to_str().expect("temporary path");

    for mode in ["--strict", "--continue"] {
        let (code, error) = check(&[mode, app]);
        assert_eq!(code, 2, "{error}");
        assert!(error.contains("roundhouse.yml"), "{error}");
    }

    std::fs::remove_dir_all(root).expect("remove temporary app");
}

#[test]
fn the_store_fixture_checks_clean() {
    let store = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/store");
    let (code, err) = check(&[store]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)"),
        "{err}"
    );
}

/// #207's original nullable ActiveRecord source-property repro must remain
/// unsupported, but survey mode must ledger the declaration and its provider.
#[test]
fn alba_rejections_are_ledgered_without_reporting_executable_support() {
    let root = std::env::temp_dir().join(format!("roundhouse-207-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    render json: {article: ArticleResource.new(Article.find(params[:id])).to_h}\n  end\nend\n"),
        ("app/resources/application_resource.rb", "class ApplicationResource\n  include Alba::Resource\nend\n"),
        ("app/resources/article_resource.rb", "class ArticleResource < ApplicationResource\n  attributes :id, :title\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("Gemfile.lock", include_str!("../fixtures/gem-capabilities/historical/locks/alba.lock")),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let path = root.to_str().unwrap();
    let (code, strict) = check(&["--strict", path]);
    assert_eq!(code, 1, "{strict}");
    assert!(strict.contains("error[unsupported]: alba_serialization"), "{strict}");
    assert!(strict.contains("0 survey gap(s)"), "{strict}");

    let (code, surveyed) = check(&["--continue", path]);
    assert_eq!(code, 1, "{surveyed}");
    assert!(surveyed.contains("error[unsupported]: alba_serialization"), "{surveyed}");
    assert!(surveyed.contains("the `alba` gem"), "{surveyed}");
    assert!(surveyed.contains("1 error(s), 0 warning(s), 0 gap-attributed note(s), 1 survey gap(s)"), "{surveyed}");
    assert!(surveyed.contains("Survey: 1 ingest gap(s)"), "{surveyed}");
    assert!(surveyed.contains("app/resources/article_resource.rb"), "{surveyed}");

    // A syntactically valid DSL outside the modeled declaration subset is
    // still unsupported. Strict mode fails fast; survey mode ledgers the
    // declaration, leaves the class unlowered, and continues to the summary.
    std::fs::write(root.join("app/resources/article_resource.rb"), "class ArticleResource < ApplicationResource\n  attributes :id, :title, if: :visible?\nend\n").unwrap();
    let (code, rejected) = check(&["--strict", path]);
    assert_eq!(code, 2, "{rejected}");
    assert!(rejected.contains("ingest failed"), "{rejected}");
    assert!(rejected.contains("Alba source-property subset: unsupported Alba declaration"), "{rejected}");
    assert!(!rejected.contains("error(s)"), "strict refusal must not print an analysis summary: {rejected}");

    let (code, continued) = check(&["--continue", path]);
    assert_ne!(code, 2, "a ledgered Alba declaration must not abort --continue: {continued}");
    assert!(!continued.contains("ingest failed"), "{continued}");
    assert!(continued.contains("Survey: 1 ingest gap(s)"), "{continued}");
    assert!(continued.contains("Alba source-property subset: unsupported Alba declaration"), "{continued}");
    assert!(continued.contains("app/resources/article_resource.rb"), "{continued}");
    assert!(continued.contains("error(s)"), "analysis summary must be printed: {continued}");
    std::fs::remove_dir_all(root).unwrap();
}

