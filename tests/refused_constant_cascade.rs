//! `X.new(…).as_json` on a constant roundhouse refuses is one gap, the
//! constant's: the Object-extension refusal on its result counted the
//! same root twice. A receiver that only reaches the constant through a
//! local keeps its refusal.

use std::process::Command;

#[test]
fn an_object_extension_on_a_refused_constant_is_not_refused_again() {
    let root = std::env::temp_dir().join(format!("roundhouse-refused-constant-cascade-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def index\n    article = Article.first\n    Nowhere::Resource.new(article).as_json\n    resource = Nowhere::Resource.new(article)\n    resource.as_json\n    head :ok\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index]\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse")).args(["check", "--continue"]).arg(&root).output().expect("spawn roundhouse");
    let out = String::from_utf8_lossy(&out.stderr).into_owned();
    let at = |pos: &str| out.lines().filter(|l| l.contains(&format!("articles_controller.rb:{pos}:"))).collect::<Vec<_>>();

    let direct = at("4:5");
    assert!(direct.iter().any(|l| l.contains("error[unsupported]: constant not supported (all targets): Nowhere::Resource")), "{out}");
    assert!(!direct.iter().any(|l| l.contains("Object extension")), "{out}");

    let through_local = at("6:5");
    assert!(through_local.iter().any(|l| l.contains("error[unsupported]: Object extension")), "{out}");

    std::fs::remove_dir_all(root).unwrap();
}
