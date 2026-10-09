//! `with_options` in `config/routes.rb` (ActiveSupport's
//! `Object#with_options`): every route call in the block receives the
//! block's options, deep-merged under its own. Mastodon spells six route
//! groups this way (`with_options to: "accounts#show" do get "/@:username",
//! as: :short_account … end`, `with_options only: [:index], concerns:
//! :batch do resources :links … end`); each was an "unsupported routes
//! DSL" gap that dropped the whole group with its helpers. (Kept out of
//! tests/emit_and_run.rs so concurrent appends there do not conflict;
//! same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::routes::flatten_routes;

fn tree(routes: &str) -> HashMap<PathBuf, Vec<u8>> {
    let mut tree = HashMap::new();
    tree.insert(PathBuf::from("config/routes.rb"), routes.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema[7.1].define(version: 1) do\nend\n".to_vec(),
    );
    tree
}

/// `VERB path -> controller#action (helper)` for every flattened route.
fn flat(routes: &str) -> Vec<String> {
    let app = ingest_app_from_tree(tree(routes)).expect("ingest tree");
    flatten_routes(&app)
        .into_iter()
        .map(|r| {
            format!(
                "{} {} -> {}#{} ({})",
                format!("{:?}", r.method).to_uppercase(),
                r.path,
                r.controller.0.as_str(),
                r.action.as_str(),
                r.as_name,
            )
        })
        .collect()
}

fn strict_error(routes: &str) -> String {
    match ingest_app_from_tree(tree(routes)) {
        Ok(_) => panic!("strict ingest accepted an unmodeled with_options shape"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn options_reach_every_call_and_the_calls_own_keys_win() {
    let got = flat(
        r#"Rails.application.routes.draw do
  with_options to: "accounts#show" do
    get "/@:username", as: :short_account
    get "/@:username/media", as: :short_account_media
    get "/@:username/raw", to: "accounts#raw", as: :raw_account
  end
end
"#,
    );
    assert!(got.contains(&"GET /@:username -> AccountsController#show (short_account)".to_string()), "{got:#?}");
    assert!(got.contains(&"GET /@:username/media -> AccountsController#show (short_account_media)".to_string()), "{got:#?}");
    assert!(got.contains(&"GET /@:username/raw -> AccountsController#raw (raw_account)".to_string()), "{got:#?}");
    assert_eq!(got.len(), 3, "{got:#?}");
}

#[test]
fn resource_options_and_nested_with_options_compose() {
    let got = flat(
        r#"Rails.application.routes.draw do
  namespace :admin do
    with_options only: [:index] do
      resources :links
      with_options controller: "feeds" do
        get "/feed", action: :show, as: :feed
      end
    end
  end
end
"#,
    );
    assert!(got.contains(&"GET /admin/links -> Admin::LinksController#index (admin_links)".to_string()), "{got:#?}");
    assert!(
        got.iter().any(|r| r.starts_with("GET /admin/feed -> Admin::FeedsController#show")),
        "{got:#?}"
    );
    // `only: [:index]` reached `resources :links`: no show/create/…
    assert!(!got.iter().any(|r| r.contains("/admin/links/:id")), "{got:#?}");
}

/// Rails merges the options into calls nested inside a child's block
/// too (the block's `self` is still the merger); that fan-out is not
/// replayed, so the child is an error rather than a route that quietly
/// lost its options.
#[test]
fn a_child_with_its_own_block_is_declined() {
    let err = strict_error(
        r#"Rails.application.routes.draw do
  with_options only: [:index] do
    resources :links do
      get :preview
    end
  end
end
"#,
    );
    assert!(err.contains("with_options: `resources` with a block is not composed"), "{err}");
}

#[test]
fn unmodeled_with_options_shapes_are_declined() {
    for (routes, detail) in [
        (
            "Rails.application.routes.draw do\n  with_options(to: \"a#b\") do |r|\n    r.get \"/x\"\n  end\nend\n",
            "a block with a parameter",
        ),
        (
            "Rails.application.routes.draw do\n  with_options OPTS do\n    get \"/x\"\n  end\nend\n",
            "options must be a literal hash",
        ),
        (
            "Rails.application.routes.draw do\n  with_options constraints: { id: /\\d+/ } do\n    get \"/x/:id\", to: \"a#b\", constraints: { slug: /x/ }\n  end\nend\n",
            "deep-merging two hashes under `constraints:`",
        ),
        (
            "Rails.application.routes.draw do\n  with_options to: \"a#b\" do\n    if ENV[\"X\"]\n      get \"/x\"\n    end\n  end\nend\n",
            "only receiverless route calls",
        ),
    ] {
        let err = strict_error(routes);
        assert!(err.contains(detail), "{routes}\n=> {err}");
    }
}

/// The emitted app serves the merged routes: the inherited target, the
/// call's own override, the inherited redirect, and the helper an `as:`
/// inside the block names.
#[test]
fn with_options_routes_are_served() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            r#"  root "articles#index"
  with_options to: "echoes#show" do
    get "/@:username", as: :short_echo
    get "/@:username/media"
    get "/@:username/raw", to: "echoes#raw"
  end
  with_options to: redirect("/articles") do
    get "/posts"
  end
"#,
        )
        .write(
            "app/controllers/echoes_controller.rb",
            "class EchoesController < ApplicationController\n  def show\n    render plain: \"show:#{params[:username]}\"\n  end\n\n  def raw\n    render plain: \"raw:#{params[:username]}\"\n  end\nend\n",
        )
        .write(
            "test/controllers/echoes_controller_test.rb",
            r##"require "test_helper"

class EchoesControllerTest < ActionDispatch::IntegrationTest
  test "with_options routes are served" do
    get "/@bob"
    assert_equal "show:bob", response.body
    get "/@ann/media"
    assert_equal "show:ann", response.body
    get "/@cy/raw"
    assert_equal "raw:cy", response.body
    assert_equal "/@dee", short_echo_path("dee")
    get "/posts"
    assert_response 301
    assert_redirected_to "/articles"
  end
end
"##,
        )
        .run_test("test/controllers/echoes_controller_test.rb")
        .assert_passes();
}
