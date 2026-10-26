//! A route constraint written as a constant: `Regexp.union(Owner::LIST)`
//! of a String list, or a regexp constant (`Owner::ID_RE`), as the class
//! constants fold them for model bodies (#1177). Before, every such value
//! was "is not a literal regex" and the route stayed out of the lowered
//! app. A compact `class Feeds::CollectionsController` resolves without a
//! file declaring `module Feeds`, which Zeitwerk defines from the
//! directory. A `format:` constant is a requirement on the extension,
//! as a literal is. Answers are actionpack 8.1.4's (`rails_answers_the_table`).

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::process::Command;

const COLLECTIONS: &str = r#"class Feeds::CollectionsController < ApplicationController
  SUPPORTED_COLLECTIONS = %w(featured tags).freeze

  def show
    render plain: "show:#{params[:id]}"
  end
end
"#;

const WIDGETS: &str = r#"class WidgetsController < ApplicationController
  ID_RE = /[0-9]+-[a-z]+/
  EXPORT_FORMAT = /csv/

  def show
    render plain: "widget:#{params[:id]}"
  end

  def export
    render plain: "export:#{params[:id]}"
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  namespace :feeds do
    resources :collections, only: [:show], constraints: { id: Regexp.union(Feeds::CollectionsController::SUPPORTED_COLLECTIONS) }
  end
  get "/widgets/:id", to: "widgets#show", constraints: { id: WidgetsController::ID_RE }
  get "/exports/:id", to: "widgets#export", constraints: { format: WidgetsController::EXPORT_FORMAT }
end
"#;

const CASES: &[(&str, &str)] = &[
    ("/feeds/collections/featured", "200|show:featured"),
    ("/feeds/collections/tags", "200|show:tags"),
    ("/feeds/collections/other", "404|"),
    ("/feeds/collections/featuredx", "404|"),
    ("/widgets/12-ab", "200|widget:12-ab"),
    ("/widgets/12", "404|"),
    // A `format:` constant is a requirement on the extension, as a
    // literal is: an extension it matches, or none, is served; any
    // other is not routed.
    ("/exports/1.csv", "200|export:1"),
    ("/exports/1", "200|export:1"),
    ("/exports/1.json", "404|"),
];

fn assertions() -> String {
    CASES
        .iter()
        .map(|(path, want)| format!("got = req({path:?})\nraise \"{path}: #{{got}}\" unless got == {want:?}\n"))
        .collect()
}

#[test]
fn the_ruby_target_folds_constant_constraints_as_rails_does() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/feeds/collections_controller.rb", COLLECTIONS)
        .write("app/controllers/widgets_controller.rb", WIDGETS)
        .write("config/routes.rb", ROUTES)
        .run_ruby(&format!(
            r#"require "stringio"
def req(path)
  result = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  result[0] == 404 ? "404|" : result[0].to_s + "|" + result[2].join
end
{}puts "PASS"
"#,
            assertions()
        ))
        .assert_passes();
}

/// A regexp constant carrying a modifier stays a gap, as a literal does.
#[test]
fn a_constant_regex_with_a_modifier_stays_a_gap() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n  ID_RE = /[a-z]+/i\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets/:id\", to: \"widgets#show\", constraints: { id: WidgetsController::ID_RE }\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let err = roundhouse::ingest::ingest_app_from_tree(tree).expect_err("a /i constant is not carried");
    assert!(err.to_string().contains("regex modifiers"), "{err}");
}

/// The table itself, against actionpack 8.1.4's router.
#[test]
fn rails_answers_the_table() {
    let probe = r#"gem "actionpack", "8.1.4"; require "action_controller""#;
    if !Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with actionpack 8.1.4 not available");
        return;
    }
    let script = format!(
        r##"gem "actionpack", "8.1.4"
require "action_controller"
require "rack/mock"
class ApplicationController < ActionController::Base; end
module Feeds; end
{COLLECTIONS}
{WIDGETS}
routes = ActionDispatch::Routing::RouteSet.new
routes.draw do
{draw}
end
define_method(:req) do |path|
  status, _headers, body = routes.call(Rack::MockRequest.env_for(path))
  next "404|" if status == 404
  text = +""
  body.each {{ |part| text << part }}
  status.to_s + "|" + text
end
{asserts}puts "Rails agrees"
"##,
        draw = ROUTES.lines().filter(|l| l.starts_with("  ")).collect::<Vec<_>>().join("\n"),
        asserts = assertions(),
    );
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "actionpack 8.1.4 disagrees with the table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

/// A controller file that is also an autoload input (an app that adds
/// `app/controllers/concerns` to its autoload paths) is recorded once:
/// a second recording counted its constant as written twice, which
/// refuses the fold.
#[test]
fn a_controller_file_on_the_autoload_paths_folds_once() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n"),
        ("config/application.rb", "module App\n  class Application < Rails::Application\n    config.autoload_paths << Rails.root.join(\"app\", \"controllers\", \"concerns\")\n  end\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/concerns/kinds.rb", "module Kinds\n  LIST = %w[featured tags].freeze\nend\n"),
        ("app/controllers/collections_controller.rb", "class CollectionsController < ApplicationController\n  def show\n    head :ok\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/collections/:id\", to: \"collections#show\", constraints: { id: Regexp.union(Kinds::LIST) }\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("the constraint folds");
}
