//! A path written inside `member do` / `collection do` nests under the
//! resource, however it is spelled.
//!
//! Measured against Rails 8.1 (`bin/rails routes`):
//!
//! ```text
//! GET /reports(/:name)                reports#show
//! GET /reports/:id/pages/:page        reports#page
//! GET /admin/reports(/:name)          admin/reports#show
//! GET /admin/stats                    admin/stats#index
//! ```
//!
//! Only a single bare segment (`get "archive"`) used to nest; a path with
//! a `:param` or an optional group was taken as absolute, so the
//! collection route came out as `/:name` and `/`, and inside a namespace
//! `/admin/:name` swallowed every later `/admin/<word>` route.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::App;
use roundhouse::dialect::HttpMethod;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::routes::{FlatRoute, flatten_routes};

fn routes(source: &str) -> Vec<FlatRoute> {
    let mut app = App::default();
    app.routes = ingest_routes(
        format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(),
        "config/routes.rb",
    )
    .expect("ingest routes");
    flatten_routes(&app)
}

fn paths(routes: &[FlatRoute], action: &str) -> Vec<String> {
    routes
        .iter()
        .filter(|r| r.method == HttpMethod::Get && r.action.as_str() == action)
        .map(|r| format!("{} {}", r.controller.0.as_str(), r.path))
        .collect()
}

#[test]
fn collection_optional_segment_keeps_the_resource_prefix() {
    let r = routes(
        r#"
  resources :reports, only: %i[index] do
    collection do
      get "/(:name)", to: "reports#show"
    end
  end
"#,
    );
    assert_eq!(
        paths(&r, "show"),
        vec!["ReportsController /reports/:name", "ReportsController /reports"],
        "{r:?}"
    );
}

#[test]
fn member_path_with_a_param_keeps_the_resource_prefix() {
    let r = routes(
        r#"
  resources :reports, only: %i[index] do
    member do
      get "pages/:page", to: "reports#page"
    end
  end
"#,
    );
    assert_eq!(paths(&r, "page"), vec!["ReportsController /reports/:id/pages/:page"], "{r:?}");
    let page = r.iter().find(|r| r.action.as_str() == "page").unwrap();
    assert_eq!(page.path_params, vec!["id".to_string(), "page".to_string()]);
}

#[test]
fn namespaced_collection_route_does_not_shadow_later_routes() {
    let r = routes(
        r#"
  namespace :admin do
    resources :reports, only: [] do
      collection do
        get "/(:name)", to: "reports#show"
      end
    end
    get "stats", to: "stats#index"
  end
"#,
    );
    assert_eq!(
        paths(&r, "show"),
        vec!["Admin::ReportsController /admin/reports/:name", "Admin::ReportsController /admin/reports"],
        "{r:?}"
    );
    // A one-segment `/admin/:name` declared first would match
    // `/admin/stats` before the stats route is reached.
    assert!(
        !r.iter().any(|x| x.path == "/admin/:name" || x.path == "/admin"),
        "a collection route escaped its resource: {r:?}"
    );
}

#[test]
fn emitted_router_dispatches_structured_member_and_collection_paths() {
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  resources :articles do
    collection do
      get "/(:name)", to: "articles#new"
    end
    member do
      get "pages/:page", to: "articles#edit"
    end
    resources :comments, only: [:create, :destroy]
  end
end
"#,
        )
        .run_ruby(
            r##"
table = RouteTable.table
[
  ["GET", "/articles/special", :new, {"name" => "special"}],
  ["GET", "/articles", :new, {}],
  ["GET", "/articles/42/pages/2", :edit, {"id" => "42", "page" => "2"}],
].each do |verb, path, action, params|
  matched = ActionDispatch::Router.match(verb, path, table)
  raise "no route for #{verb} #{path}" if matched.nil?
  raise "#{verb} #{path}: #{matched.action}, #{matched.path_params}" unless
    matched.action == action && matched.path_params == params
end
["/special", "/pages/2"].each do |path|
  matched = ActionDispatch::Router.match("GET", path, table)
  raise "GET #{path} escaped its resource: #{matched.action}" unless matched.nil?
end
puts "PASS emitted member/collection paths"
"##,
        )
        .assert_passes();
}
