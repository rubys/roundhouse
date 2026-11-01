//! A resource whose singular and plural are the same word (`following`,
//! `news`) names its collection routes with `_index`, as Rails does:
//! `news_index` is `/news` and `news` is `/news/:id`. Roundhouse named
//! the index `news` too, so the show route's helper was lost to the
//! clash, and a call to `account_following_index_url` named no helper.
//!
//! Expected names and paths are Rails 8.1.4's for the same routes
//! (actionpack 8.1.4, oracle `rails-oracle.rb`): a nested uncountable,
//! a nested full one, a top-level one with a collection and a member
//! route, a namespaced one, and a countable control.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ROUTES: &str = "Rails.application.routes.draw do\n  # (a) a nested uncountable, index only, and a nested full resource\n  resources :accounts, only: [:show] do\n    resources :following, only: [:index]\n    resources :news\n  end\n  # (b) a top-level uncountable with index and show\n  # (d) member and collection routes on it\n  resources :news, only: [:index, :show] do\n    collection { get :latest }\n    member { get :preview }\n  end\n  # (c) namespaced\n  namespace :admin do\n    resources :news, only: [:index, :show, :new, :edit]\n  end\n  # (e) a countable control\n  resources :widgets, only: [:index, :show, :new, :edit]\nend\n";

const ASSERTIONS: &str = r#"
require_relative "app/route_helpers"
got = [
  RouteHelpers.account_following_index_path(7),
  RouteHelpers.account_news_index_path(7),
  RouteHelpers.new_account_news_path(7),
  RouteHelpers.edit_account_news_path(7, 3),
  RouteHelpers.account_news_path(7, 3),
  RouteHelpers.latest_news_index_path,
  RouteHelpers.preview_news_path(3),
  RouteHelpers.news_index_path,
  RouteHelpers.news_path(3),
  RouteHelpers.admin_news_index_path,
  RouteHelpers.new_admin_news_path,
  RouteHelpers.edit_admin_news_path(3),
  RouteHelpers.admin_news_path(3),
  RouteHelpers.widgets_path,
  RouteHelpers.new_widget_path,
  RouteHelpers.edit_widget_path(3),
  RouteHelpers.widget_path(3)
]
want = [
  "/accounts/7/following",
  "/accounts/7/news",
  "/accounts/7/news/new",
  "/accounts/7/news/3/edit",
  "/accounts/7/news/3",
  "/news/latest",
  "/news/3/preview",
  "/news",
  "/news/3",
  "/admin/news",
  "/admin/news/new",
  "/admin/news/3/edit",
  "/admin/news/3",
  "/widgets",
  "/widgets/new",
  "/widgets/3/edit",
  "/widgets/3"
]
raise got.zip(want).reject { |g, w| g == w }.inspect unless got == want
puts "uncountable routes passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", ROUTES)
}

#[test]
fn an_uncountable_resource_names_its_collection_routes_with_index_on_ruby() {
    let run = example().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("uncountable routes passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_uncountable_resource_names_its_collection_routes_with_index_on_spinel_with_rbs() {
    let run = example().run_spinel_with_rbs(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("uncountable routes passed"), "{}", run.stdout);
}
