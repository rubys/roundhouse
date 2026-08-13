//! A record handed to a path helper becomes its param at the CALL SITE
//! (`emit::ruby::library::apply_route_param_lowering`).
//!
//! The pass used to bail entirely unless some model overrode `to_param`.
//! lobsters has `Tag#to_param`, so it ran there and nowhere else — and
//! the first app to arrive without one (campfire) rendered records
//! straight into URLs: `redirect_to room_url(...)` produced
//! `Location: /rooms/#<Room:0x0000000121836fe0>`.
//!
//! The default `to_param` IS `id`, and `routes_to_library::param_ty`
//! declares a non-slug `id` segment `Int`, so a plain model's record
//! converts with `.id` while a slug model's uses `.to_param`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
    t.integer "user_id", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "username", null: false
  end
end
"#,
        ),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  belongs_to :user\nend\n",
        ),
        (
            // Overrides to_param — the shape lobsters' Tag/Comment have.
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_many :rooms

  def to_param
    username
  end
end
"#,
        ),
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  resources :rooms
  resources :users
end
"#,
        ),
        (
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def index
    @room = Room.first
    redirect_to room_path(current_user.rooms.last)
  end

  def show
    @room = Room.find(params[:id])
    @plain = room_path(@room)
    @slug = user_path(@room.user)
    @already = user_path(@room.user.username)
  end

  private
    def current_user
      User.first
    end
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn shown() -> String {
    let files = ruby::emit_lowered_controllers(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/controllers/rooms_controller.rb"))
        .map(|f| f.content.clone())
        .expect("rooms_controller.rb")
}

/// The regression this pass existed for but never ran on: a model with
/// no `to_param` override converts with `.id`, matching the `Int` its
/// path segment is declared.
#[test]
fn a_plain_models_record_converts_with_id() {
    let c = shown();
    assert!(
        c.contains("RouteHelpers.room_path(@room.id)"),
        "@room becomes @room.id:\n{c}"
    );
}

/// A model that DOES override `to_param` keeps the slug call — the
/// original behavior, still reached through a singular association read.
#[test]
fn a_slug_models_record_still_converts_with_to_param() {
    let c = shown();
    assert!(
        c.contains("RouteHelpers.user_path(@room.user.to_param)"),
        "a to_param-overriding model uses to_param:\n{c}"
    );
}

/// `.first`/`.last` on a has_many answers one record of the collection's
/// type. campfire's front door is `room_url(Current.user.rooms.last)`,
/// which no name-based signal sees.
#[test]
fn first_or_last_on_a_has_many_converts() {
    let c = shown();
    assert!(
        c.contains("RouteHelpers.room_path(self.current_user.rooms.last.id)")
            || c.contains("RouteHelpers.room_path(current_user.rooms.last.id)"),
        "a has_many .last converts:\n{c}"
    );
}

/// Positive-signal-only: an argument that is ALREADY a slug is left
/// exactly as written. Wrapping it would be the bug in the other
/// direction.
#[test]
fn an_already_slug_argument_is_untouched() {
    let c = shown();
    assert!(
        c.contains("RouteHelpers.user_path(@room.user.username)"),
        "an explicit slug read stays as written:\n{c}"
    );
    assert!(
        !c.contains("username.id") && !c.contains("username.to_param"),
        "no double conversion:\n{c}"
    );
}

// ── inflection + emitted-path agreement ──────────────────────────────
//
// Not route-param lowering, but the same failure family: a path the emit
// COMPUTES has to name the file the emit WROTE. Three separate call
// sites derived one from `snake_case` (which passes `::` through) or from
// a pluralizer that turned `key` into `keies`, and every one of them
// produced a require for a file that does not exist. CRuby only notices
// at dispatch; the spinel lane resolves requires at build time and fails
// the whole build, which is how these surfaced.

/// `underscore` nests on `::`; `snake_case` does not. A namespaced
/// controller lives at `app/controllers/accounts/users_controller.rb`.
#[test]
fn a_namespaced_class_underscores_to_a_nested_path() {
    assert_eq!(
        roundhouse::naming::underscore("Accounts::Bots::KeysController"),
        "accounts/bots/keys_controller"
    );
    assert!(
        !roundhouse::naming::underscore("Accounts::UsersController").contains("::"),
        "no `::` survives into a path"
    );
}

/// A trailing `y` becomes `ies` only after a CONSONANT. `key` → `keys`,
/// so campfire's `resource :key` names `Accounts::Bots::KeysController`
/// and its require finds the file the controller emitter wrote.
#[test]
fn a_vowel_before_y_pluralizes_with_s() {
    for (one, many) in [
        ("key", "keys"),
        ("day", "days"),
        ("boy", "boys"),
        ("survey", "surveys"),
    ] {
        assert_eq!(roundhouse::naming::pluralize_snake(one), many, "{one}");
    }
}

/// The consonant case is unchanged — this is a narrowing, not a rewrite.
#[test]
fn a_consonant_before_y_still_pluralizes_with_ies() {
    for (one, many) in [
        ("story", "stories"),
        ("category", "categories"),
        ("reply", "replies"),
        ("activity", "activities"),
    ] {
        assert_eq!(roundhouse::naming::pluralize_snake(one), many, "{one}");
    }
}
