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
use roundhouse::emit::rust;
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

fn request_app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n  attribute :request\n  delegate :host, :protocol, to: :request, prefix: true, allow_nil: true\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  before_action do\n    Current.request = request\n  end\nend\n"),
        ("app/controllers/rooms_controller.rb", "class RoomsController < ApplicationController\n  def index\n  end\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest request app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn session_current_app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/models/current.rb",
            "class Current < ActiveSupport::CurrentAttributes\n  attribute :session, :user\n  def session=(value)\n    super(value)\n    if value.present?\n      self.user = session.user\n    end\n  end\nend\n",
        ),
        (
            "app/models/session.rb",
            "class Session\n  def user\n    User.first\n  end\nend\n",
        ),
        ("app/models/user.rb", USER),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  before_action do\n    Current.session = Session.new\n  end\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def index\n  end\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest session Current tree");
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

#[test]
fn a_nullable_class_method_nil_predicate_uses_option_semantics() {
    let app = app();
    let files = rust::emit(&app);
    let source = files
        .iter()
        .find(|file| file.path.to_string_lossy().ends_with("controllers/rooms_controller.rs"))
        .expect("Rust rooms controller")
        .content
        .as_str();
    assert!(
        source.contains("Current::user().is_none()")
            && !source.contains("Current::user().is_null()"),
        "a nullable Current.user return is an Option, not serde_json::Value:\n{source}"
    );
}

#[test]
fn current_attribute_writer_clones_the_option_without_unwrapping_it() {
    let app = app();
    let files = rust::emit(&app);
    let source = files
        .iter()
        .find(|file| file.path.to_string_lossy().ends_with("app_classes/current_class.rs"))
        .expect("Rust Current class")
        .content
        .as_str();
    let writer = source
        .split("pub fn __current_instance_set_user")
        .nth(1)
        .and_then(|method| method.split("\n    }").next())
        .expect("Current.user writer");
    assert!(
        writer.contains("self.user = value.clone()")
            && !writer.contains("value.clone().unwrap()"),
        "an Option-valued CurrentAttributes writer must preserve nil:\n{writer}"
    );
}

#[test]
fn a_current_writer_conditional_preserves_an_option_returning_branch() {
    let app = session_current_app();
    let files = rust::emit(&app);
    let source = files
        .iter()
        .find(|file| file.path.to_string_lossy().ends_with("app_classes/current_class.rs"))
        .expect("Rust Current class")
        .content
        .as_str();
    let writer = source
        .split("pub fn __current_instance_set_session")
        .nth(1)
        .and_then(|method| method.split("\n    }").next())
        .expect("Current.session writer");
    assert!(
        writer.contains("self.__current_instance_set_user")
            && !writer.contains("Some(self.__current_instance_set_user"),
        "a branch already returning Option<User> must not become Option<Option<User>>:\n{writer}"
    );
}

/// Ablation for the thread-local read: `Thread#[]` answers untyped, and
/// a reader that returned it on a mere `nil?` test signed its type as
/// `untyped` — spinel then dispatched every forwarder on a boxed value.
#[test]
fn the_instance_reader_narrows_the_thread_local_to_the_class() {
    let sig = emitted("current.rbs");
    assert!(sig.contains("def self.instance: () -> Current"), "{sig}");
}

/// A setter synthesized from CurrentAttributes must keep the type of its
/// whole-app write sites all the way through the Rust class forwarder.
#[test]
fn request_writer_uses_the_type_from_its_controller_filter_write() {
    let app = request_app();
    let files = rust::emit(&app);
    let source = files
        .iter()
        .find(|file| file.path.to_string_lossy().ends_with("current_class.rs"))
        .expect("Rust Current class")
        .content
        .as_str();

    assert!(
        source.contains("pub request: Option<crate::http::RequestContext>"),
        "CurrentAttributes request slots must remain nilable before setup writes:\n{source}"
    );
    assert!(
        source.contains("__current_instance_set_request(&mut self, value: crate::http::RequestContext)"),
        "{source}"
    );
    assert!(
        source.contains("pub fn set_request(value: crate::http::RequestContext)")
            && source.contains("__current_instance_set_request(value)"),
        "{source}"
    );
    assert!(
        source.contains("unwrap().host()"),
        "a present Current.request must be unwrapped before delegated access:\n{source}"
    );
    let delegated_host = source
        .split("pub fn __current_instance_request_host")
        .nth(1)
        .and_then(|method| method.split("\n    }").next())
        .expect("generated request_host delegate");
    assert!(
        delegated_host.contains(".is_none()") && !delegated_host.contains("unwrap().is_none()"),
        "allow_nil must test the Option before unwrapping its value:\n{delegated_host}"
    );
    assert!(
        files.iter().any(|file| {
            file.path.to_string_lossy().contains("controllers/")
                && file
                    .content
                    .contains("crate::http::current_request_context()")
        }),
        "controller request setup must read the active request context"
    );
}
