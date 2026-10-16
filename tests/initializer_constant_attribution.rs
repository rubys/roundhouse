//! A constant an initializer assigns is defined at boot, so the app's
//! reads of it are sound; with no home for it in the ingested tree yet,
//! those reads are coverage notes naming the initializer, while an
//! unrelated or namespaced namesake stays an error.

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
fn reads_of_a_constant_an_initializer_assigns_are_coverage_notes() {
    let root = std::env::temp_dir().join(format!("roundhouse-initializer-constants-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("config/initializers/000-stats.rb", "::STATS = Object.new\nclass Rack::Attack\n  ADMIN_ROLES = []\nend\n"),
        ("config/initializers/010-stats.rb", "STATS = Object.new\n"),
        ("config/initializers/020-maybe.rb", "if ENV[\"FEATURE\"]\n  MAYBE = Object.new\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def index\n    STATS.inspect\n    Rack::Attack::ADMIN_ROLES.inspect\n    MAYBE.inspect\n    Nope.inspect\n    Admin::STATS.inspect\n    head :ok\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index]\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let out = check_continue(&root);
    let line = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no `{needle}` line:\n{out}"));

    let stats = line("constant not supported (all targets): STATS");
    assert!(stats.contains("note[unsupported]"), "{stats}");
    assert!(stats.contains("assigned in config/initializers/000-stats.rb, config/initializers/010-stats.rb"), "{stats}");
    // Assigned in a class body, which runs with the file.
    let roles = line("constant not supported (all targets): Rack::Attack::ADMIN_ROLES");
    assert!(roles.contains("note[unsupported]") && roles.contains("000-stats.rb"), "{roles}");

    // Assigned only when a condition holds: it may be undefined at run time.
    let maybe = line("constant not supported (all targets): MAYBE");
    assert!(maybe.contains("error[unsupported]"), "{maybe}");

    // Not assigned anywhere: an error of its own.
    let nope = line("constant not supported (all targets): Nope");
    assert!(nope.contains("error[unsupported]"), "{nope}");
    // The initializer assigns the top-level `STATS`, not `Admin::STATS`.
    let admin = line("constant not supported (all targets): Admin::STATS");
    assert!(admin.contains("error[unsupported]"), "{admin}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_initializer_class_named_only_in_a_comment_is_not_kept() {
    let root = std::env::temp_dir().join(format!("roundhouse-initializer-comment-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        // Kept, its body would report the constant nothing defines.
        ("config/initializers/legacy.rb", "class LegacyHook\n  def self.run\n    NoSuchConstantAnywhere.call\n  end\nend\n"),
        ("app/models/article.rb", "# LegacyHook used to run here.\nclass Article < ApplicationRecord\n  def label = \"LegacyHook\"\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let out = check_continue(&root);
    assert!(out.contains(&format!("roundhouse-check: {} —", root.display())), "check did not complete:\n{out}");
    assert!(!out.contains("NoSuchConstantAnywhere"), "LegacyHook was kept:\n{out}");

    // Named as code, rooted, it is kept, and its body reports.
    std::fs::write(
        root.join("app/models/article.rb"),
        "class Article < ApplicationRecord\n  def label = ::LegacyHook.run\nend\n",
    )
    .unwrap();
    let out = check_continue(&root);
    assert!(out.contains("NoSuchConstantAnywhere"), "rooted `::LegacyHook` did not keep it:\n{out}");
    std::fs::remove_dir_all(root).unwrap();
}
