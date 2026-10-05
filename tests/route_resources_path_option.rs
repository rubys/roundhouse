//! `resources :name, path: "segment"` serves the resource at the segment
//! and keeps the helpers and controller from the name.
//!
//! Measured against Rails 8.1 (`bin/rails routes`):
//!
//! ```text
//!          parts GET /components                parts#index
//!           part GET /components/:id            parts#show
//! archive_widget GET /gadgets/:id/archive       widgets#archive
//!   widget_parts GET /gadgets/:widget_id/parts  parts#index
//!         widget GET /gadgets/:id               widgets#show
//! ```
//!
//! The option used to be ignored, so every one of these was served at
//! `/parts` / `/widgets` and the declared paths answered 404.

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

/// `as_name path controller#action` for every GET route, in table order.
fn table(routes: &[FlatRoute]) -> Vec<String> {
    routes
        .iter()
        .filter(|r| r.method == HttpMethod::Get)
        .map(|r| format!("{} {} {}#{}", r.as_name, r.path, r.controller.0.as_str(), r.action.as_str()))
        .collect()
}

#[test]
fn path_option_moves_the_url_and_keeps_the_names() {
    let r = routes(r#"  resources :parts, path: "/components", only: %i[index show]"#);
    assert_eq!(
        table(&r),
        vec![
            "parts /components PartsController#index",
            "part /components/:id PartsController#show",
        ]
    );
}

#[test]
fn members_and_nested_resources_sit_under_the_path_segment() {
    let r = routes(
        r#"
  resources :widgets, path: "gadgets", only: %i[show] do
    member { get :archive }
    resources :parts, only: %i[index]
  end
"#,
    );
    assert_eq!(
        table(&r),
        vec![
            "archive_widget /gadgets/:id/archive WidgetsController#archive",
            "widget_parts /gadgets/:widget_id/parts PartsController#index",
            "widget /gadgets/:id WidgetsController#show",
        ]
    );
}

#[test]
fn emitted_router_serves_the_resource_at_its_path() {
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  resources :articles, path: "posts" do
    resources :comments, only: [:create, :destroy]
  end
end
"#,
        )
        .run_ruby(
            r##"
table = RouteTable.table
[
  ["GET", "/posts", :index, {}],
  ["GET", "/posts/42", :show, {"id" => "42"}],
  ["POST", "/posts/42/comments", :create, {"article_id" => "42"}],
].each do |verb, path, action, params|
  matched = ActionDispatch::Router.match(verb, path, table)
  raise "no route for #{verb} #{path}" if matched.nil?
  raise "#{verb} #{path}: #{matched.action}, #{matched.path_params}" unless
    matched.action == action && matched.path_params == params
end
raise "/articles is still routed" unless ActionDispatch::Router.match("GET", "/articles", table).nil?
raise "article_path(42) is #{RouteHelpers.article_path(42)}" unless RouteHelpers.article_path(42) == "/posts/42"
puts "PASS emitted resources path:"
"##,
        )
        .assert_passes();
}

#[test]
fn non_literal_path_is_unsupported_not_the_resource_name() {
    for value in ["PARTS_SEGMENT", "segment_for(:parts)"] {
        let err = ingest_routes(
            format!("Rails.application.routes.draw do\n  resources :parts, path: {value}\nend\n")
                .as_bytes(),
            "config/routes.rb",
        )
        .map(|t| format!("ingested: {t:?}"))
        .expect_err("non-literal path: should be unsupported");
        assert!(err.to_string().contains("resources :parts `path:` is not a literal"), "{value}: {err}");
    }
}

#[test]
fn dynamic_segment_path_is_unsupported_not_served_without_its_param() {
    for value in [r#""categories/:category_id/parts""#, r#""files/*rest""#, r#""parts(/:kind)""#] {
        let err = ingest_routes(
            format!("Rails.application.routes.draw do\n  resources :parts, path: {value}\nend\n")
                .as_bytes(),
            "config/routes.rb",
        )
        .map(|t| format!("ingested: {t:?}"))
        .expect_err("dynamic path: should be unsupported");
        assert!(err.to_string().contains("has a dynamic segment"), "{value}: {err}");
    }
}

#[test]
fn empty_path_is_unsupported_not_the_resource_name() {
    // Rails mounts `path: ""` / `path: "/"` at the root (`GET /` is
    // `parts#index`), not at `/parts`.
    for value in [r#""""#, r#""/""#, r#""//""#] {
        let err = ingest_routes(
            format!("Rails.application.routes.draw do\n  resources :parts, path: {value}\nend\n")
                .as_bytes(),
            "config/routes.rb",
        )
        .map(|t| format!("ingested: {t:?}"))
        .expect_err("empty path: should be unsupported");
        assert!(err.to_string().contains("mounts the resource at the root"), "{value}: {err}");
    }
}
