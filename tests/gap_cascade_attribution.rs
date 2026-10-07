//! A class survey mode drops for an ingest gap leaves every reference to it
//! unresolved. Those shadows read as coverage notes naming the gap, the way
//! an unknown gem's do, while an unrelated error stays an error.

use std::process::Command;

fn check_continue(root: &std::path::Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(root)
        .output()
        .expect("spawn roundhouse");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn references_to_a_class_dropped_by_an_ingest_gap_are_coverage_notes() {
    let root = std::env::temp_dir().join(format!("roundhouse-gap-cascade-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        // `has_role?` is a gem's method: the visibility change refuses the whole model.
        ("app/models/article.rb", "class Article < ApplicationRecord\n  LIMIT = 5\n  private :has_role?\nend\n"),
        // Sorts first, but Rails autoloads `Article` from `app/models/article.rb`: that is the cause named.
        ("app/lib/early_patch.rb", "class Article < ApplicationRecord\n  private :has_role?\nend\n"),
        ("app/models/admin/summary.rb", "class Admin::Summary < ApplicationRecord\n  private :has_role?\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def show\n    @article = Article.find(params[:id])\n    Nope.call\n    ::Summary.call\n    Article::LIMIT\n  end\n\n  def index\n    @headline = latest_article.headline\n  end\n\n  private\n\n  def latest_article\n    nil\n  end\nend\n",
        ),
        // The dropped class still names a type: a value typed `Article?` fails dispatch.
        ("sig/articles_controller.rbs", "class ArticlesController\n  def latest_article: () -> Article?\nend\n"),
        ("app/views/articles/show.html.erb", "<h1><%= @article.title %></h1>\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index, :show]\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let out = check_continue(&root);
    // One run passes by luck when the cause is picked in HashMap order; that order changes per process.
    for _ in 0..4 {
        assert_eq!(check_continue(&root), out, "the output differs between runs");
    }
    let line = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no `{needle}` line:\n{out}"));

    let article = line("constant not supported (all targets): Article");
    assert!(article.contains("note[unsupported]"), "{article}");
    assert!(article.contains("ingest gap in app/models/article.rb"), "{article}");

    let value = line("constant not supported (all targets): Article::LIMIT");
    assert!(value.contains("note[unsupported]"), "{value}");
    assert!(value.contains("ingest gap in app/models/article.rb"), "{value}");

    let ivar = line("@article has no known type");
    assert!(ivar.contains("note[ivar_unresolved]"), "{ivar}");
    assert!(ivar.contains("@article is assigned from a class whose source did not ingest"), "{ivar}");

    let dispatch = line("no known method `headline`");
    assert!(dispatch.contains("note[send_dispatch_failed]"), "{dispatch}");
    assert!(dispatch.contains("ingest gap in app/models/article.rb"), "{dispatch}");

    // Declared by no gap file: an error of its own, not a shadow.
    let nope = line("constant not supported (all targets): Nope");
    assert!(nope.contains("error[unsupported]"), "{nope}");
    // A gap file declares `Admin::Summary`, which is not the top-level `::Summary`.
    let summary = line("constant not supported (all targets): Summary");
    assert!(summary.contains("error[unsupported]"), "{summary}");

    std::fs::remove_dir_all(root).unwrap();
}
