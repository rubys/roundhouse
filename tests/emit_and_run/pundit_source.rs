use std::path::Path;

use roundhouse::project::BuildTarget;

use super::emit_and_run;
use super::pundit_app::app;

#[test]
fn authorize_and_policy_scope_run_from_the_gem_source() {
    app()
        .run_ruby(
            r##"
def get(path, query = "")
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  [status, body.join]
end
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
published = Article.create!(title: "Published", body: "body number one is long enough")
Article.create!(title: "Hidden draft", body: "body number two is long enough")
check("edit as anyone", get("/articles/#{published.id}/edit")[0], 403)
check("edit as admin", get("/articles/#{published.id}/edit", "user=admin")[0], 200)
_, body = get("/articles")
check("index as anyone", [body.include?("Published"), body.include?("Hidden draft")], [true, false])
_, body = get("/articles", "user=admin")
check("index as admin", [body.include?("Published"), body.include?("Hidden draft")], [true, true])
_, body = get("/articles", "user=nobody")
check("index as nobody", [body.include?("Published"), body.include?("Hidden draft")], [false, false])
check("rooted name", RoundhouseConstants.safe_constantize("::ArticlePolicy"), ArticlePolicy)
puts "ok"
"##,
        )
        .assert_passes();
}

#[test]
fn safe_constantize_is_a_closed_case_on_the_ruby_family_and_refused_elsewhere() {
    let dir = std::env::temp_dir().join(format!("rh-constantize-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/post_policy.rb", "class PostPolicy\nend\n"),
        (
            "app/models/finder.rb",
            "class Finder\n  def self.policy(name)\n    \"#{name}Policy\".safe_constantize\n  end\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    Finder.policy(\"Post\")\n    render plain: \"ok\"\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let emit = |target: BuildTarget| {
        let (files, diags) =
            roundhouse::emit::diagnostics::scope(|| roundhouse::project::target_files(&app, &dir, target));
        let errors: Vec<String> = diags
            .into_iter()
            .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
            .map(|d| d.message)
            .collect();
        (errors, files.unwrap_or_default())
    };
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (errors, files) = emit(target);
        assert!(!errors.iter().any(|e| e.contains("safe_constantize")), "{target:?} refused the call:\n{}", errors.join("\n"));
        let text = |file: &str| files.iter().find(|(path, _)| path.ends_with(file)).map(|(_, text)| text.clone()).unwrap_or_default();
        let finder = text("finder.rb");
        assert!(finder.contains("RoundhouseConstants.safe_constantize("), "{target:?} did not route the call to the lookup:\n{finder}");
        let lookup = text("roundhouse_constants.rb");
        assert!(lookup.contains("when \"PostPolicy\""), "{target:?} did not emit the closed case:\n{lookup}");
    }
    for target in [BuildTarget::Typescript, BuildTarget::Rust, BuildTarget::Python] {
        let (errors, _) = emit(target);
        assert!(
            errors.iter().any(|e| e.contains("safe_constantize")),
            "{target:?} did not refuse safe_constantize:\n{}",
            errors.join("\n")
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_what_require_loads_is_ingested() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "errors: {errors:#?}");
    let models = emitted.join("app/models/pundit");
    assert!(models.join("policy_finder.rb").exists(), "the required gem files were not emitted");
    for unrequired in ["rspec.rb", "policy_generator.rb"] {
        assert!(!walk(&emitted).iter().any(|p| p.ends_with(unrequired)), "{unrequired} was ingested");
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
