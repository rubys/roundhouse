//! `class Current < ActiveSupport::CurrentAttributes` — the class every
//! per-request read in campfire goes through, and the one the analyzer
//! could see nothing inside.
//!
//! Three separate reasons it was shapeless, each of which had to go:
//!
//! 1. **The writes are outside the class.** `Current.session = session`
//!    is app code; the only write the class's own syntax shows is the
//!    generated `def session=(value); @session = value; end`, whose
//!    parameter has no type. The seed comes from surveying the app.
//! 2. **`reset` nilled the singleton.** `@__instance`'s type is the
//!    union of what the class assigns it, so one `= nil` made
//!    `self.instance` answer `Current | Nil` and every class-level
//!    forwarder register `Untyped`. `reset` now REPLACES the instance,
//!    which is what resetting a CurrentAttributes MEANS.
//! 3. **The forwarder's body cannot type itself.** `Ty::Class { Current }`
//!    is both the class object and an instance here, and the
//!    class-method table is consulted first, so `Current.instance.user`
//!    looks up the forwarder it is in the middle of computing. The
//!    answer is copied from the instance twin instead.
//!
//! And the Nil arm STAYS. That is the fourth test: `signed_in?` is
//! `Current.user.present?`, and against a non-nilable type it folds to
//! `true` — a correct fold of an incorrect type. Stripping nil here (as
//! the controller-wide ivar seed does) signed everyone in.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\n  \
    create_table :rooms do |t|\n    t.integer :user_id\n    t.string :name\n  end\nend\n";

const CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :user
end
"#;

const USER: &str = r#"
class User < ApplicationRecord
  has_many :rooms
end
"#;

const ROOM: &str = "class Room < ApplicationRecord\n  belongs_to :user\nend\n";

const CONTROLLER: &str = r#"
class RoomsController < ApplicationController
  def index
    Current.user = User.first
    @rooms = Current.user.rooms
  end

  def show
    head :forbidden unless signed_in?
  end

  private
    def signed_in?
      Current.user.present?
    end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
        ("app/models/current.rb", CURRENT),
        ("app/models/user.rb", USER),
        ("app/models/room.rb", ROOM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn errors(needle: &str) -> Vec<String> {
    diagnose(&app())
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains(needle))
        .collect()
}

fn emitted(stem: &str) -> String {
    let app = app();
    // `emit_spinel` emits the lowered models/controllers/views;
    // `Current` is a LIBRARY class and rides the other half.
    let mut files = ruby::emit_spinel(&app);
    files.extend(ruby::emit_library(&app));
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(stem))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ends with {stem}; got {:?}",
                files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
            )
        })
}

/// The whole point: a read through the class-level forwarder answers a
/// model, so the hop after it dispatches.
#[test]
fn a_read_through_the_forwarder_answers_the_model() {
    let o = errors("rooms");
    assert!(
        o.is_empty(),
        "`Current.user.rooms` must dispatch — the write site says `User`: {o:?}"
    );
}

/// Ablation for reason 2: `reset` must not nil the slot, or
/// `self.instance` answers `Current | Nil` and nothing downstream types.
#[test]
fn reset_replaces_the_instance_rather_than_nilling_it() {
    let src = emitted("app/models/current.rb");
    assert!(
        src.contains("Thread.current[:__current_attrs_Current] = Current.new")
            && !src.contains("Thread.current[:__current_attrs_Current] = nil"),
        "resetting a CurrentAttributes means a fresh instance:\n{src}"
    );
}

/// Ablation for the Nil arm: `present?` on a nilable model folds to a
/// nil CHECK. Folding it to `true` is what signing everyone in looks
/// like, so assert the constant is not there.
#[test]
fn the_nil_arm_survives_so_presence_is_still_asked() {
    let src = emitted("app/controllers/rooms_controller.rb");
    assert!(
        src.contains("Current.user.nil?"),
        "`Current.user.present?` must still ask; it must not fold to true:\n{src}"
    );
}
