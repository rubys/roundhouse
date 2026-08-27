//! A bare `new` in a class body is the class's own constructor.
//!
//! campfire's `Sound::BUILTIN` is an array of fifty-six of them, and
//! the emit replayed the bare spelling into a strict target, where a
//! receiverless call has no receiver to resolve against and `new` is
//! not a free function anywhere:
//!
//! ```text
//! app/models/sound.rb:39: unsupported call:
//!   node 95214 (CallNode `new`) recv=-/ty-1 argc=1
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn emitted(poro: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/models/sound.rb", poro),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("sound.rb"))
        .map(|f| f.content.clone())
        .expect("sound.rb")
}

const SOUND: &str = r#"class Sound
  attr_reader :name

  def initialize(name:)
    @name = name
  end

  BUILTIN = [ new(name: "bell"), new(name: "honk") ]

  def self.first_builtin
    new(name: "first")
  end
end
"#;

#[test]
fn a_constant_initializers_bare_new_names_its_class() {
    let src = emitted(SOUND);
    assert!(
        src.contains("BUILTIN = [ Sound.new(name: \"bell\"), Sound.new(name: \"honk\") ]"),
        "a class-body `new` is the class's own:\n{src}"
    );
}

/// `def self.` is the same implicit self, one line down.
#[test]
fn a_class_side_method_body_gets_it_too() {
    let src = emitted(SOUND);
    assert!(
        src.contains("Sound.new(name: \"first\")"),
        "a class-side body's bare `new` is the class's own:\n{src}"
    );
}

/// An INSTANCE method's bare `new` is a NoMethodError in Ruby too —
/// there is nothing there to preserve, and nothing to rewrite.
#[test]
fn an_instance_method_is_left_alone() {
    let src = emitted(
        "class Sound\n  def initialize(name: nil)\n    @name = name\n  end\n\n  \
         def sibling\n    Sound.new(name: \"x\")\n  end\nend\n",
    );
    assert!(
        src.contains("Sound.new(name: \"x\")") && !src.contains("Sound.new(Sound"),
        "an explicit constructor stays exactly one constructor:\n{src}"
    );
}

/// THE ORDERING IS PART OF THE FIX. `partition_deferred_constants`
/// holds back a constant whose initializer dispatches to its own class
/// until after the methods are defined — a class body runs top to
/// bottom, so `Sound.new` above `def initialize` is
/// `BasicObject#initialize: wrong number of arguments` at LOAD time.
/// That rule keyed on the bare `new` spelling alone, so the receiver
/// this pass adds made it stop seeing campfire's `Sound::BUILTIN`: the
/// unit assertions above still passed and all 52 files of the campfire
/// suite went to zero.
#[test]
fn a_self_constructing_constant_still_defers_past_the_methods() {
    let src = emitted(SOUND);
    let builtin = src.find("BUILTIN =").expect("BUILTIN emitted");
    let init = src.find("def initialize").expect("initialize emitted");
    assert!(
        builtin > init,
        "a constant that constructs its own class must be emitted AFTER \
         the methods it calls:\n{src}"
    );
}
