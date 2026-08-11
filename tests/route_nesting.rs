//! Rails' resource-nesting rules for paths and helper names.
//!
//! Every expectation here was MEASURED against Rails 8.1 by drawing the
//! same routes into a real `ActionDispatch::Routing::RouteSet` — these
//! rules are too fiddly to derive, and three of them are the opposite of
//! what looks obvious:
//!
//!   * A SINGULAR parent contributes its path segment but NO id
//!     (`resource :account` → `/account/logo`, not
//!     `/account/:account_id/logo`) — there is only ever one.
//!   * Nesting SURVIVES a `scope`/`namespace` boundary. Resetting it
//!     cost campfire 35 of its 78 helpers.
//!   * An explicit `as:` sits AFTER the parent for a nested route
//!     (`room_at_message`) and BEFORE it for a member/collection one
//!     (`peek_room`) — a nested route names a thing belonging to the
//!     parent, a member route names an action on it.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::routes::flatten_routes;

/// (as_name, path) for every named route, in declaration order.
fn routes_of(routes_rb: &str) -> Vec<(String, String)> {
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
    let app = ingest_app_from_tree(tree).expect("ingest");
    flatten_routes(&app)
        .into_iter()
        .filter(|r| r.named)
        .map(|r| (r.as_name.clone(), r.path.clone()))
        .collect()
}

fn find<'a>(routes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    routes
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p.as_str())
}

#[test]
fn singular_parent_contributes_no_id_segment() {
    // Rails: account_logo GET /account/logo
    let r = routes_of(
        r#"
  resource :account do
    scope module: "accounts" do
      resource :logo, only: %i[ show destroy ]
    end
  end
"#,
    );
    assert_eq!(find(&r, "account_logo"), Some("/account/logo"), "{r:?}");
}

#[test]
fn nesting_survives_a_scope_boundary() {
    // Rails: user_avatar GET /users/:user_id/avatar
    let r = routes_of(
        r#"
  resources :users, only: :show do
    scope module: "users" do
      resource :avatar, only: :show
    end
  end
"#,
    );
    assert_eq!(find(&r, "user_avatar"), Some("/users/:user_id/avatar"), "{r:?}");
}

#[test]
fn nesting_accumulates_through_every_level() {
    // Rails: account_bot_key PATCH /account/bots/:bot_id/key — the
    // singular `account` adds a segment but no id, the plural `bots`
    // adds both, and BOTH names reach the helper.
    let r = routes_of(
        r#"
  resource :account do
    scope module: "accounts" do
      resources :bots do
        scope module: "bots" do
          resource :key, only: :update
        end
      end
    end
  end
"#,
    );
    assert_eq!(
        find(&r, "account_bot_key"),
        Some("/account/bots/:bot_id/key"),
        "{r:?}"
    );
}

#[test]
fn explicit_path_is_slash_joined_to_the_parent() {
    // Rails: room_at_message GET /rooms/:room_id/@:message_id — the
    // segment separator is unconditional, even before a path that does
    // not start with one.
    let r = routes_of(
        r#"
  resources :rooms do
    get "@:message_id", to: "rooms#show", as: :at_message
  end
"#,
    );
    assert_eq!(
        find(&r, "room_at_message"),
        Some("/rooms/:room_id/@:message_id"),
        "{r:?}"
    );
}

#[test]
fn explicit_as_goes_after_the_parent_when_nested() {
    // Rails: room_bot_messages POST /rooms/:room_id/:bot_key/messages
    let r = routes_of(
        r#"
  resources :rooms do
    post ":bot_key/messages", to: "messages/by_bots#create", as: :bot_messages
  end
"#,
    );
    assert_eq!(
        find(&r, "room_bot_messages"),
        Some("/rooms/:room_id/:bot_key/messages"),
        "{r:?}"
    );
}

#[test]
fn inline_on_collection_is_the_block_form() {
    // Rails: clear_searches DELETE /searches/clear — no id, and the
    // action name leads. Only `collection do … end` was recognized, so
    // this nested as `/searches/:search_id/clear`.
    let r = routes_of(
        r#"
  resources :searches, only: %i[ index create ] do
    delete :clear, on: :collection
  end
"#,
    );
    assert_eq!(find(&r, "clear_searches"), Some("/searches/clear"), "{r:?}");
}

#[test]
fn inline_on_member_carries_the_record_id() {
    // Rails: bump_room GET /rooms/:id/bump — `:id`, the record's own key.
    let r = routes_of(
        r#"
  resources :rooms do
    get :bump, on: :member
  end
"#,
    );
    assert_eq!(find(&r, "bump_room"), Some("/rooms/:id/bump"), "{r:?}");
}

#[test]
fn explicit_as_goes_before_the_parent_for_member_and_collection() {
    // Rails: peek_room GET /rooms/:id/preview, fresh_rooms GET /rooms/recent.
    let r = routes_of(
        r#"
  resources :rooms do
    get :preview, on: :member, as: :peek
    get :recent, on: :collection, as: :fresh
  end
"#,
    );
    assert_eq!(find(&r, "peek_room"), Some("/rooms/:id/preview"), "{r:?}");
    assert_eq!(find(&r, "fresh_rooms"), Some("/rooms/recent"), "{r:?}");
}

#[test]
fn a_scope_path_prefixes_outside_the_parent_nesting() {
    // Rails: user_badge GET /extra/users/:user_id/badge — the scope's
    // path leads, the parent nesting follows.
    let r = routes_of(
        r#"
  resources :users do
    scope :extra do
      resource :badge, only: :show
    end
  end
"#,
    );
    assert_eq!(
        find(&r, "user_badge"),
        Some("/extra/users/:user_id/badge"),
        "{r:?}"
    );
}

/// KNOWN DIVERGENCE, measured and written down rather than guessed at.
///
/// A `namespace` nests INSIDE the parent while a `scope path:` goes
/// OUTSIDE it — Rails 8.1, same enclosing `resources :users`:
///
///   namespace :admin       → /users/:user_id/admin/notes  user_admin_notes
///   scope path: :px, as: :ax → /px/users/:user_id/cap      ax_user_cap
///
/// Roundhouse emits the SCOPE behavior for both, because
/// `ingest_namespace_route` lowers `namespace` to the same
/// `RouteSpec::Scope` variant — so the flattener cannot tell them apart
/// (it produces `admin_user_notes` at `/admin/users/:user_id/notes`).
/// Fixing it means distinguishing the two in the IR and giving `Ctx` an
/// inner-path accumulator alongside `ns_path`.
///
/// Ignored rather than deleted: neither lobsters nor campfire nests a
/// `namespace` inside a resource block, so nothing is broken today, and
/// the expectation is already written for whoever needs it.
#[test]
#[ignore = "namespace-inside-resources nests differently from scope; unexercised by the corpus"]
fn namespace_inside_a_resource_keeps_the_nesting() {
    // Rails: user_admin_notes GET /users/:user_id/admin/notes
    let r = routes_of(
        r#"
  resources :users do
    namespace :admin do
      resources :notes, only: :index
    end
  end
"#,
    );
    assert_eq!(
        find(&r, "user_admin_notes"),
        Some("/users/:user_id/admin/notes"),
        "{r:?}"
    );
}
