//! `scope defaults: { user_id: "me" }` makes a segment's helper param
//! OPTIONAL (`lower::routes` → `routes_to_library::build_helper_function`).
//!
//! Rails fills a defaulted dynamic segment in when the caller omits it,
//! so campfire's `resource :profile` under that scope answers
//! `user_profile_url` with NO argument even though its path is
//! `/users/:user_id/profile`. Ingest used to drop `defaults:` as
//! something that "shapes the request, not the (path, controller,
//! action) triple" — true of the triple, false of the SIGNATURE, and
//! four of campfire's controller tests died on `wrong number of
//! arguments (given 0, expected 1)`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::{ParamKind, Ty};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

fn helper_params(routes: &str, name: &str) -> Vec<roundhouse::ty::Param> {
    let files = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("config/routes.rb", routes),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let helpers = lower_routes_to_library_functions(&app);
    let f = helpers
        .iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "helper {name} not generated; got: {:?}",
                helpers.iter().map(|f| f.name.as_str().to_string()).collect::<Vec<_>>()
            )
        });
    let Ty::Fn { params, .. } = f.signature.clone().expect("signature") else {
        panic!("not a Ty::Fn")
    };
    params
}

/// The shape campfire has, verbatim.
const CAMPFIRE_SHAPE: &str = r#"
Rails.application.routes.draw do
  resources :users, only: :show do
    scope module: "users" do
      scope defaults: { user_id: "me" } do
        resource :profile
      end
    end
  end
end
"#;

#[test]
fn a_defaulted_segment_is_an_optional_helper_param() {
    let params = helper_params(CAMPFIRE_SHAPE, "user_profile_path");
    assert_eq!(params.len(), 1, "the segment is still a parameter: {params:?}");
    assert_eq!(params[0].name.as_str(), "user_id");
    assert_eq!(
        params[0].kind,
        ParamKind::Optional,
        "a defaulted segment must be callable with no argument: {params:?}"
    );
}

#[test]
fn an_undefaulted_segment_stays_required() {
    let params = helper_params(
        r#"
Rails.application.routes.draw do
  resources :users, only: :show do
    scope module: "users" do
      resource :profile
    end
  end
end
"#,
        "user_profile_path",
    );
    assert_eq!(params.len(), 1);
    assert_eq!(
        params[0].kind,
        ParamKind::Required,
        "without `defaults:` the segment is still required: {params:?}"
    );
}

/// The default applies only inside the scope that declares it.
#[test]
fn the_default_does_not_leak_to_a_sibling_scope() {
    let params = helper_params(
        r#"
Rails.application.routes.draw do
  resources :users, only: :show do
    scope module: "users" do
      scope defaults: { user_id: "me" } do
        resource :profile
      end
      resource :avatar
    end
  end
end
"#,
        "user_avatar_path",
    );
    assert_eq!(params.len(), 1);
    assert_eq!(
        params[0].kind,
        ParamKind::Required,
        "the sibling resource declares no default: {params:?}"
    );
}

/// A defaulted segment that is NOT the SUFFIX becomes a KEYWORD, and
/// leaves the positional list entirely.
///
/// campfire routes push_subscriptions under the same scope, so Rails
/// accepts `user_push_subscription_path(record)` — one argument for a
/// two-segment member route. Keeping `user_id` a positional-with-a-
/// default is what Ruby would do and what the strict targets cannot:
/// Rust has no default arguments, so the emitter fills them at the CALL
/// SITE by padding MISSING TRAILING args, and a LEADING default would be
/// appended at the end instead of filled in place — silently swapping
/// the segments in the URL.
///
/// The cost, and the direction is the point: Rails also accepts
/// `user_push_subscription_path(user, record)`. Against a keyword that
/// is an ARITY ERROR — loud, at the call site, naming the helper —
/// rather than a URL with its segments swapped.
const NESTED_MEMBER_SHAPE: &str = r#"
Rails.application.routes.draw do
  resources :users, only: :show do
    scope module: "users" do
      scope defaults: { user_id: "me" } do
        resources :push_subscriptions
      end
    end
  end
end
"#;

#[test]
fn a_leading_defaulted_segment_becomes_a_keyword() {
    let params = helper_params(NESTED_MEMBER_SHAPE, "user_push_subscription_path");
    assert_eq!(params.len(), 2, "both segments are still parameters: {params:?}");
    assert_eq!(params[0].name.as_str(), "id", "the undefaulted segment leads: {params:?}");
    assert_eq!(params[0].kind, ParamKind::Required);
    assert_eq!(params[1].name.as_str(), "user_id");
    assert_eq!(
        params[1].kind,
        ParamKind::Keyword { required: false },
        "the defaulted segment is a keyword, not a positional: {params:?}"
    );
}

/// …and when the defaults ARE the suffix, nothing moves: the collection
/// helper under the same scope keeps its single optional positional.
#[test]
fn a_trailing_defaulted_segment_stays_positional() {
    let params = helper_params(NESTED_MEMBER_SHAPE, "user_push_subscriptions_path");
    assert_eq!(params.len(), 1, "{params:?}");
    assert_eq!(params[0].name.as_str(), "user_id");
    assert_eq!(params[0].kind, ParamKind::Optional, "{params:?}");
}
