//! A record reaches a path helper through a chain NOTHING TYPED
//! (`emit::ruby::library::apply_route_param_lowering`).
//!
//! campfire's welcome page is `redirect_to room_path(last_room_visited)`,
//! and every link in what that names is untyped: `Current.user` is an
//! ivar on a lowered CurrentAttributes class, `rooms` is a `has_many
//! :through` with no foreign key to seed from, and the method itself is
//! an inherited controller method. So the RECORD was interpolated into
//! the path and the redirect went to `/rooms/#<Rooms::Open:0x…>`.
//!
//! None of that needs the chain typed. Each rung below is a fact the
//! compiler already holds, read off a NAME rather than a type, and each
//! is held to the same uniqueness rule the association maps are: a name
//! two declarations disagree about answers None.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(app_controller_body: &str, welcome_body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    t.string :name\n  end\n  \
             create_table :rooms do |t|\n    t.string :name\n    t.string :created_at\n  end\n  \
             create_table :notes do |t|\n    t.string :body\n    t.string :created_at\n  end\n  \
             create_table :memberships do |t|\n    t.integer :user_id\n    t.integer :room_id\n  end\nend\n"
                .to_string(),
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
             has_many :memberships\n  \
             has_many :rooms, through: :memberships\n  \
             has_many :notes\nend\n"
                .to_string(),
        ),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  \
             has_many :memberships\n  \
             def self.original\n    order(:created_at).first\n  end\n  \
             def self.newest\n    order(:created_at).last\n  end\nend\n"
                .to_string(),
        ),
        // A second model declaring `newest`, so THAT class-method name
        // answers None while `original` stays Room's alone — the two
        // sides of the uniqueness rule, in one fixture.
        (
            "app/models/note.rb",
            "class Note < ApplicationRecord\n  \
             belongs_to :user\n  \
             def self.newest\n    order(:created_at).last\n  end\nend\n"
                .to_string(),
        ),
        (
            "app/models/membership.rb",
            "class Membership < ApplicationRecord\n  belongs_to :user\n  belongs_to :room\nend\n"
                .to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            format!("class ApplicationController < ActionController::Base\n{app_controller_body}end\n"),
        ),
        (
            "app/controllers/welcome_controller.rb",
            format!("class WelcomeController < ApplicationController\n{welcome_body}end\n"),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\n  resources :notes\n  \
             get \"welcome\", to: \"welcome#index\"\nend\n"
                .to_string(),
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    ruby::emit_lowered_controllers(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// campfire's shape, end to end. Three rungs have to hold at once:
///
///  * `find_by` on a has_many read answers ONE record of the
///    collection's type — the same fact `.first`/`.last` already carried,
///    and split out only because those take no arguments;
///  * `a || b` answers a model when BOTH sides do and they agree;
///  * a bare (or `self.`) call to a method declared ANYWHERE IN THIS
///    SLICE resolves through what that method's body answers — the
///    caller is in `WelcomeController` and the method in
///    `ApplicationController`.
#[test]
fn a_record_reached_through_an_untyped_chain_still_projects_to_its_id() {
    let out = emit(
        "  def last_room_visited\n    \
           current_user.rooms.find_by(id: 1) || self.default_room\n  end\n  \
         def default_room\n    current_user.rooms.original\n  end\n  \
         def current_user\n    User.first\n  end\n",
        "  def index\n    redirect_to RouteHelpers.room_path(last_room_visited)\n  end\n",
    );
    assert!(
        out.contains("RouteHelpers.room_path(last_room_visited.id)"),
        "the record must project to its id:\n{out}"
    );
}

/// The `||` rung requires BOTH sides to answer the SAME model. One side
/// resolving proves nothing about the value that actually arrives.
#[test]
fn an_or_whose_sides_disagree_resolves_to_nothing() {
    let out = emit(
        "  def either\n    current_user.rooms.find_by(id: 1) || current_user.notes.find_by(id: 1)\n  end\n  \
         def current_user\n    User.first\n  end\n",
        "  def index\n    redirect_to RouteHelpers.room_path(either)\n  end\n",
    );
    assert!(
        out.contains("RouteHelpers.room_path(either)"),
        "disagreeing sides must not project:\n{out}"
    );
    assert!(
        !out.contains("either.id"),
        "disagreeing sides must not project:\n{out}"
    );
}

/// A class-method NAME two models declare answers nothing — the same
/// standard `collection_association_targets` holds association names to.
/// Both `Room` and `Note` define `newest`, so a read through it cannot
/// say which one arrives; `original` is Room's alone and does resolve,
/// which is what the first test above rides.
#[test]
fn an_ambiguous_class_method_name_resolves_to_nothing() {
    let out = emit(
        "  def ambiguous\n    current_user.rooms.newest\n  end\n  \
         def current_user\n    User.first\n  end\n",
        "  def index\n    redirect_to RouteHelpers.room_path(ambiguous)\n  end\n",
    );
    assert!(
        out.contains("RouteHelpers.room_path(ambiguous)"),
        "an ambiguous class-method name must not project:\n{out}"
    );
    assert!(!out.contains("ambiguous.id"), "must not project:\n{out}");
}

/// A method whose body answers something this pass cannot classify is
/// left alone — the rule stays positive-signal-only.
#[test]
fn a_method_answering_an_unclassifiable_value_is_left_alone() {
    let out = emit(
        "  def whatever\n    params[:anything]\n  end\n",
        "  def index\n    redirect_to RouteHelpers.room_path(whatever)\n  end\n",
    );
    assert!(
        out.contains("RouteHelpers.room_path(whatever)"),
        "an unclassifiable body must not project:\n{out}"
    );
    assert!(!out.contains("whatever.id"), "must not project:\n{out}");
}
