//! Routes whose `(.:format)` extension is held to a requirement, and the
//! answer Rails 8.1.4 gives each request (`tests/routes_format_requirement.rs`
//! checks Rails itself and the Ruby target against the same table).

use super::emit_and_run::{self, Overlay};

pub const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def export
    render plain: "export:#{params[:format]}"
  end

  def raw
    render plain: "raw:#{params[:format]}"
  end

  # Not `params[:format]`: Rails seeds it from the route's default, which
  # the lowered dispatch does not yet do (the forced format only picks the
  # response format there).
  def pinned
    render plain: "pinned"
  end

  def member
    render plain: "member:#{params[:format]}"
  end
end
"#;

pub const ROUTES: &str = r#"Rails.application.routes.draw do
  get "/exports/:id/export", to: "widgets#export", constraints: { format: /csv/ }
  get "/raw/:id", to: "widgets#raw", format: false
  get "/pinned/:id", to: "widgets#pinned", format: "json"
  get "/members/:id", to: "widgets#member", constraints: { format: /json|xml/ }
end
"#;

/// `(path, "status|body")`; a 404 carries no body here.
pub const CASES: &[(&str, &str)] = &[
    // A requirement holds an extension to it; no extension always passes.
    ("/exports/1/export", "200|export:"),
    ("/exports/1/export.csv", "200|export:csv"),
    ("/exports/1/export.json", "404|"),
    // `format: false`: no extension at all.
    ("/raw/1", "200|raw:"),
    ("/raw/1.json", "404|"),
    // `format: "json"`: a requirement and the default both.
    ("/pinned/1", "200|pinned"),
    ("/pinned/1.json", "200|pinned"),
    ("/pinned/1.csv", "404|"),
    // A member route: the extension is never bound into `:id`.
    ("/members/1", "200|member:"),
    ("/members/1.xml", "200|member:xml"),
    ("/members/1.json", "200|member:json"),
    ("/members/1.csv", "404|"),
];

pub fn overlay() -> Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("config/routes.rb", ROUTES)
}

/// A Ruby script asserting `CASES` through `req(path)`, which the caller
/// defines to answer `"status|body"`.
pub fn assertions() -> String {
    let mut s = String::new();
    for (path, expected) in CASES {
        s.push_str(&format!("got = req({path:?})\nraise \"{path}: #{{got}}\" unless got == {expected:?}\n"));
    }
    s
}

/// Constraints written as plain text, which Rails matches exactly
/// (`literal_overlay`).
pub const LITERAL_CONTROLLER: &str = r#"class ExportsController < ApplicationController
  def index
    render plain: "index:#{params[:format]}"
  end

  def followers
    render plain: "followers:#{params[:format]}"
  end

  def download
    render plain: "download:#{params[:format]}"
  end

  def kind
    render plain: "kind:#{params[:kind]}"
  end
end
"#;

pub const LITERAL_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :exports, only: :index, constraints: { format: :json }
  constraints(format: :csv) do
    get "/exports/:id/followers", to: "exports#followers"
  end
  resources :exports, only: [] do
    member { get :download, constraints: { format: "csv" } }
  end
  get "/kinds/:kind", to: "exports#kind", constraints: { kind: "main-feed" }
end
"#;

pub const LITERAL_CASES: &[(&str, &str)] = &[
    ("/exports", "200|index:"),
    ("/exports.json", "200|index:json"),
    ("/exports.csv", "404|"),
    ("/exports/1/followers", "200|followers:"),
    ("/exports/1/followers.csv", "200|followers:csv"),
    ("/exports/1/followers.CSV", "404|"),
    ("/exports/1/followers.json", "404|"),
    ("/exports/1/download", "200|download:"),
    ("/exports/1/download.csv", "200|download:csv"),
    ("/exports/1/download.json", "404|"),
    // Exact and case-sensitive.
    ("/kinds/main-feed", "200|kind:main-feed"),
    ("/kinds/main-feeds", "404|"),
    ("/kinds/Main-feed", "404|"),
];

pub fn literal_overlay() -> Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/exports_controller.rb", LITERAL_CONTROLLER)
        .write("config/routes.rb", LITERAL_ROUTES)
}

pub fn literal_assertions() -> String {
    let mut s = String::new();
    for (path, expected) in LITERAL_CASES {
        s.push_str(&format!("got = req({path:?})\nraise \"{path}: #{{got}}\" unless got == {expected:?}\n"));
    }
    s
}
