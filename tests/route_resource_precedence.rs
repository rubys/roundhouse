//! Resource blocks register their declarations before their generated defaults.
//!
//! Measured against Rails 8.1.3.1: collection routes must precede `/:id`,
//! and an explicit member route must precede the default it overrides.

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

fn position(routes: &[FlatRoute], method: HttpMethod, path: &str, action: &str) -> usize {
    routes
        .iter()
        .position(|route| {
            route.method == method && route.path == path && route.action.as_str() == action
        })
        .unwrap_or_else(|| panic!("missing {method:?} {path} => {action}: {routes:?}"))
}

#[test]
fn collection_block_and_inline_routes_precede_member_defaults() {
    let r = routes(
        r#"
  resources :transactions do
    collection do
      get :search
      patch :bulk_update
    end
    delete :clear, on: :collection
  end
"#,
    );
    assert!(
        position(&r, HttpMethod::Get, "/transactions/search", "search")
            < position(&r, HttpMethod::Get, "/transactions/:id", "show")
    );
    assert!(
        position(
            &r,
            HttpMethod::Patch,
            "/transactions/bulk_update",
            "bulk_update"
        ) < position(&r, HttpMethod::Patch, "/transactions/:id", "update")
    );
    assert!(
        position(&r, HttpMethod::Delete, "/transactions/clear", "clear")
            < position(&r, HttpMethod::Delete, "/transactions/:id", "destroy")
    );
}

#[test]
fn explicit_member_overrides_precede_defaults() {
    let r = routes(
        r#"
  resources :transactions do
    get :edit, on: :member, to: "transactions#preview"
  end
"#,
    );
    assert!(
        position(&r, HttpMethod::Get, "/transactions/:id/edit", "preview")
            < position(&r, HttpMethod::Get, "/transactions/:id/edit", "edit")
    );
}

#[test]
fn ordering_applies_inside_nested_resources_and_scopes() {
    let r = routes(
        r#"
  namespace :admin do
    resources :accounts, only: :show do
      resources :transactions, only: [:show, :update] do
        get :search, on: :collection
        patch :bulk_update, on: :collection
      end
    end
  end
"#,
    );
    assert!(
        position(
            &r,
            HttpMethod::Get,
            "/admin/accounts/:account_id/transactions/search",
            "search"
        ) < position(
            &r,
            HttpMethod::Get,
            "/admin/accounts/:account_id/transactions/:id",
            "show"
        )
    );
    assert!(
        position(
            &r,
            HttpMethod::Patch,
            "/admin/accounts/:account_id/transactions/bulk_update",
            "bulk_update"
        ) < position(
            &r,
            HttpMethod::Patch,
            "/admin/accounts/:account_id/transactions/:id",
            "update"
        )
    );
}

#[test]
fn only_and_except_filter_defaults_without_dropping_children() {
    let r = routes(
        r#"
  resources :transactions, only: [] do
    get :search, on: :collection
  end
  resources :receipts, except: [:show, :update] do
    get :search, on: :collection
  end
  resource :profile, only: :show do
    resource :avatar, only: :show
  end
"#,
    );
    assert_eq!(
        r.iter()
            .filter(|route| route.controller.0.as_str() == "TransactionsController")
            .count(),
        1
    );
    assert!(
        !r.iter()
            .any(|route| route.controller.0.as_str() == "ReceiptsController"
                && matches!(route.action.as_str(), "show" | "update"))
    );
    assert!(
        position(&r, HttpMethod::Get, "/receipts/search", "search")
            < position(&r, HttpMethod::Get, "/receipts", "index")
    );
    assert!(
        position(&r, HttpMethod::Get, "/profile/avatar", "show")
            < position(&r, HttpMethod::Get, "/profile", "show")
    );
}

#[test]
fn ordering_keeps_top_level_and_child_declaration_precedence() {
    let r = routes(
        r#"
  get "/transactions/:id", to: "transactions#earlier"
  resources :transactions do
    get :search, on: :collection, to: "transactions#first"
    get :search, on: :collection, to: "transactions#second"
  end
  get "/transactions/search", to: "transactions#later"
"#,
    );
    assert!(
        position(&r, HttpMethod::Get, "/transactions/:id", "earlier")
            < position(&r, HttpMethod::Get, "/transactions/search", "first")
    );
    assert!(
        position(&r, HttpMethod::Get, "/transactions/search", "first")
            < position(&r, HttpMethod::Get, "/transactions/search", "second")
    );
    assert!(
        position(&r, HttpMethod::Get, "/transactions/:id", "show")
            < position(&r, HttpMethod::Get, "/transactions/search", "later")
    );
}

#[test]
fn emitted_router_dispatches_collection_actions_and_member_overrides() {
    // Use existing fixture actions as distinct destinations so this proves
    // route selection without adding unrelated controller/runtime behavior.
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  resources :articles do
    collection do
      get :search, to: "articles#index"
      patch :bulk_update, to: "articles#create"
      put :bulk_update, to: "articles#create"
      delete :clear, to: "articles#create"
    end
    get :edit, on: :member, to: "articles#index"
    resources :comments, only: [:create, :destroy] do
      get :search, on: :collection, to: "comments#create"
      delete :clear, on: :collection, to: "comments#create"
    end
  end
end
"#,
        )
        .run_ruby(
            r##"
table = RouteTable.table
[
  ["GET", "/articles/search", :index, {}],
  ["GET", "/articles/new", :new, {}],
  ["PATCH", "/articles/bulk_update", :create, {}],
  ["PUT", "/articles/bulk_update", :create, {}],
  ["DELETE", "/articles/clear", :create, {}],
  ["GET", "/articles/42/edit", :index, {"id" => "42"}],
  ["GET", "/articles/42", :show, {"id" => "42"}],
  ["PATCH", "/articles/42", :update, {"id" => "42"}],
  ["PUT", "/articles/42", :update, {"id" => "42"}],
  ["GET", "/articles/42/comments/search", :create, {"article_id" => "42"}],
  ["DELETE", "/articles/42/comments/clear", :create, {"article_id" => "42"}],
  ["DELETE", "/articles/42/comments/3", :destroy, {"article_id" => "42", "id" => "3"}],
].each do |verb, path, action, params|
  matched = ActionDispatch::Router.match(verb, path, table)
  raise "no route for #{verb} #{path}" if matched.nil?
  raise "#{verb} #{path}: #{matched.action}, #{matched.path_params}" unless
    matched.action == action && matched.path_params == params
end
puts "PASS emitted resource route precedence"
"##,
        )
        .assert_passes();
}
