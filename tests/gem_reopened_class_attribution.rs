//! A serializer's `as_json` comes from the serializer gem, which parks
//! its base under the framework's `ActiveModel` namespace. With the gem
//! unmodeled, the call is a coverage note naming it, also when the app
//! reopens that base; an app class the gem does not define keeps the
//! Object-extension error.

use std::process::Command;

fn check_continue(root: &std::path::Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(root)
        .output()
        .expect("spawn roundhouse");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write_app(root: &std::path::Path, extra: &[(&str, &str)]) {
    let base = [
        ("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"active_model_serializers\"\ngem \"sanitize\"\n"),
        (
            "Gemfile.lock",
            "GEM\n  remote: https://rubygems.org/\n  specs:\n    active_model_serializers (0.10.14)\n    rails (8.1.0)\n    sanitize (7.0.0)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  active_model_serializers\n  rails\n  sanitize\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/serializers/application_serializer.rb", "class ApplicationSerializer < ActiveModel::Serializer\nend\n"),
        (
            "app/serializers/article_serializer.rb",
            "class ArticleSerializer < ApplicationSerializer\n  attributes :id, :title\n\n  def author\n    ArticleSerializer.new(object, root: false).as_json\n  end\nend\n",
        ),
        ("lib/sanitize/app_config.rb", "class Sanitize::AppConfig\n  def to_h\n    {}\n  end\nend\n"),
        ("app/models/report.rb", "class Report\n  def summary\n    \"report\"\n  end\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def index\n    article = Article.first\n    ArticleSerializer.new(article).as_json\n    Report.new.as_json\n    Sanitize::AppConfig.new.as_json\n    head :ok\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index]\nend\n"),
    ];
    for (path, source) in base.iter().chain(extra) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
}

fn assert_attribution(out: &str) {
    let line = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no `{needle}` line:\n{out}"));

    let in_body = line("article_serializer.rb:5:5");
    assert!(in_body.contains("note[send_dispatch_failed]") && in_body.contains("`active_model_serializers`"), "{in_body}");
    let in_action = line("articles_controller.rb:4:5");
    assert!(in_action.contains("note[send_dispatch_failed]") && in_action.contains("`active_model_serializers`"), "{in_action}");
    assert!(!out.contains("articles_controller.rb:4:5: error"), "{out}");

    // No gem behind these: the app's own class, and one the app nests in a gem's namespace.
    let report = line("articles_controller.rb:5:5");
    assert!(report.contains("error[unsupported]: Object extension"), "{report}");
    let nested = line("articles_controller.rb:6:5");
    assert!(nested.contains("error[unsupported]: Object extension"), "{nested}");
}

#[test]
fn as_json_on_a_serializer_is_a_note_naming_the_gem() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-serializer-{}", std::process::id()));
    write_app(&root, &[]);
    assert_attribution(&check_continue(&root));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn as_json_on_a_serializer_stays_a_note_when_the_app_reopens_its_base() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-serializer-reopened-{}", std::process::id()));
    write_app(
        &root,
        &[("lib/patches/serializer_include.rb", "module ActiveModel\n  class Serializer\n    def include!(name)\n      name\n    end\n  end\nend\n")],
    );
    assert_attribution(&check_continue(&root));
    std::fs::remove_dir_all(root).unwrap();
}
