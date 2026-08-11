//! `direct :name do |…| route_for :target, … end` → a real
//! `RouteHelpers.<name>_path` (`lower::routes_to_library::direct`).
//!
//! `direct` is a custom URL helper, not a route — it adds nothing to the
//! dispatch table and its body is arbitrary Ruby. Ingest dropped it with
//! a ledger line, so campfire's layout called `fresh_account_logo_path`
//! and raised NameError on every page.
//!
//! The query-string rules below were MEASURED against Rails 8.1, not
//! assumed: `direct` + `url_for` + `Hash#to_query` compose in ways that
//! are hard to predict, and two of the three are surprising.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::routes_to_library::lower_routes_to_library_functions;
use roundhouse::App;

fn app_with(routes_rb: &str) -> App {
    let tree = vec![
        (
            std::path::PathBuf::from("config/routes.rb"),
            format!("Rails.application.routes.draw do\n{routes_rb}\nend\n")
                .as_bytes()
                .to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/thing.rb"),
            b"class Thing < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

/// The emitted Ruby for one generated helper.
fn helper_src(routes_rb: &str, name: &str) -> String {
    let app = app_with(routes_rb);
    let funcs = lower_routes_to_library_functions(&app);
    let f = funcs
        .iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} not generated; got: {:?}",
                funcs.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
            )
        });
    format!("{:?}", f.body)
}

const AVATAR_ROUTES: &str = r#"
  resources :users, only: :show do
    scope module: "users" do
      resource :avatar, only: :show
    end
  end
"#;

#[test]
fn direct_helper_is_generated_at_all() {
    let app = app_with(&format!(
        "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :user_avatar, user.avatar_token\n  end\n"
    ));
    let names: Vec<String> = lower_routes_to_library_functions(&app)
        .iter()
        .map(|f| f.name.as_str().to_string())
        .collect();
    assert!(
        names.contains(&"fresh_user_avatar_path".to_string()),
        "{names:?}"
    );
}

#[test]
fn trailing_options_param_takes_an_empty_hash_default() {
    // Rails calls the block with the helper's args PLUS an options hash,
    // so `fresh_account_logo_path` with NO arguments must still bind
    // `options`. Campfire's layout calls it bare.
    let app = app_with(
        r#"
  resource :account do
    scope module: "accounts" do
      resource :logo, only: :show
    end
  end
  direct :fresh_account_logo do |options|
    route_for :account_logo
  end
"#,
    );
    let funcs = lower_routes_to_library_functions(&app);
    let f = funcs
        .iter()
        .find(|f| f.name.as_str() == "fresh_account_logo_path")
        .expect("generated");
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name.as_str(), "options");
    assert!(
        f.params[0].default.is_some(),
        "the options param needs a default so a bare call binds it"
    );
}

#[test]
fn route_for_resolves_to_the_target_path_helper() {
    let src = helper_src(
        &format!(
            "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :user_avatar, user.avatar_token\n  end\n"
        ),
        "fresh_user_avatar_path",
    );
    assert!(src.contains("user_avatar_path"), "{src}");
    assert!(src.contains("avatar_token"), "positional arg lost:\n{src}");
    // No query keys — no array machinery at all.
    assert!(!src.contains("__q"), "no query means no builder:\n{src}");
}

#[test]
fn query_keys_are_sorted_alphabetically_not_in_written_order() {
    // Rails' `Hash#to_query` sorts: `v: …, size: …` emits `size` first.
    let src = helper_src(
        &format!(
            "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :user_avatar, user.avatar_token, v: user.updated_at, size: options[:size]\n  end\n"
        ),
        "fresh_user_avatar_path",
    );
    let size_at = src.find("size=").expect("size key");
    let v_at = src.find("\"v=").expect("v key");
    assert!(
        size_at < v_at,
        "keys must be sorted, not written order:\n{src}"
    );
}

#[test]
fn a_nil_valued_key_is_dropped_not_rendered_empty() {
    // Rails omits the key entirely; `?size=` is a different URL.
    let src = helper_src(
        &format!(
            "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :user_avatar, user.avatar_token, size: options[:size]\n  end\n"
        ),
        "fresh_user_avatar_path",
    );
    assert!(src.contains("nil?"), "expected a per-key nil guard:\n{src}");
    assert!(src.contains("url_encode"), "value must be encoded:\n{src}");
}

#[test]
fn each_value_is_evaluated_exactly_once() {
    // The nil guard and the rendered piece must share one evaluation —
    // campfire's value is a safe-navigation chain the desugar expands,
    // and testing it twice would run it twice.
    let src = helper_src(
        &format!(
            "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :user_avatar, user.avatar_token, v: user.updated_at\n  end\n"
        ),
        "fresh_user_avatar_path",
    );
    assert_eq!(
        src.matches("updated_at").count(),
        1,
        "value should be bound to a temporary, not re-evaluated:\n{src}"
    );
}

#[test]
fn route_for_naming_an_unknown_route_is_left_alone() {
    // Emitting a call to a helper nobody defines would surface as a
    // mystery NameError; leaving it names the missing route instead.
    let src = helper_src(
        &format!(
            "{AVATAR_ROUTES}\n  direct :fresh_user_avatar do |user, options|\n    route_for :no_such_route, user\n  end\n"
        ),
        "fresh_user_avatar_path",
    );
    assert!(src.contains("route_for"), "call should survive:\n{src}");
    assert!(!src.contains("no_such_route_path"), "{src}");
}
