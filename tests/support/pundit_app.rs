//! real-blog with pundit 2.5.2 mounted from its source as a
//! `source_gems` entry, one policy with a Scope, shared by the CRuby
//! and Spinel suites.

use std::path::Path;

use super::emit_and_run;

pub const GEM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pundit_gem");

const APPLICATION_POLICY: &str = "class ApplicationPolicy
  attr_reader :user, :record

  def initialize(user, record)
    @user = user
    @record = record
  end

  def edit?
    false
  end

  class Scope
    def initialize(user, scope)
      @user = user
      @scope = scope
    end

    def resolve
      raise NoMethodError, \"You must define #resolve in #{self.class}\"
    end

    private

    attr_reader :user, :scope
  end
end
";

const ARTICLE_POLICY: &str = "class ArticlePolicy < ApplicationPolicy
  def edit?
    user == \"admin\" && record.class.exists?(id: record.id)
  end

  class Scope < ApplicationPolicy::Scope
    def resolve
      return scope.none if user == \"nobody\"
      return scope.all if user == \"admin\"
      scope.where(title: \"Published\").or(scope.where.not(title: \"Hidden draft\"))
    end
  end
end
";

fn with_gem(mut overlay: emit_and_run::Overlay, dir: &Path, rel: &str) -> emit_and_run::Overlay {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read pundit_gem").map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap();
        let rel = format!("{rel}/{name}");
        overlay = if path.is_dir() {
            with_gem(overlay, &path, &rel)
        } else {
            overlay.write(&rel, &std::fs::read_to_string(&path).unwrap())
        };
    }
    overlay
}

pub fn app() -> emit_and_run::Overlay {
    let overlay = emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    pundit (2.5.2)\n      activesupport (>= 3.0.0)\n    debug (1.11.1)\n",
        )
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  pundit\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"pundit\"}\n")
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  include Pundit::Authorization\n\n\
             \x20 rescue_from Pundit::NotAuthorizedError do\n    head :forbidden\n  end\n\n\
             \x20 def current_user\n    params[:user]\n  end\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "@articles = Article.includes(:comments).order(created_at: :desc)",
            "@articles = policy_scope(Article).order(created_at: :desc)",
        )
        .edit("app/controllers/articles_controller.rb", "  def edit\n  end", "  def edit\n    authorize @article\n  end")
        .write("app/policies/application_policy.rb", APPLICATION_POLICY)
        .write("app/policies/article_policy.rb", ARTICLE_POLICY);
    with_gem(overlay, Path::new(GEM), "vendor/bundle/ruby/3.4.0/gems/pundit-2.5.2")
}

