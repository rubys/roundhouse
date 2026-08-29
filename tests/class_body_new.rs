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
    emitted_files(&[("app/models/sound.rb", poro)])
        .into_iter()
        .find(|(p, _)| p.ends_with("sound.rb"))
        .map(|(_, c)| c)
        .expect("sound.rb")
}

/// Every emitted library file, for the tests that need to look at a
/// class OTHER than the one they wrote the constructor in.
fn emitted_files(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ];
    files.extend_from_slice(extra);
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content.clone()))
        .collect()
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

/// THE OWNER IS NOT ALWAYS `self`. In a class-side method a SUBCLASS
/// inherits, `self` at call time is the subclass — the class-side
/// template method, which campfire writes as
/// `ActionText::Content::Filter`:
///
/// ```ruby
/// class Filter
///   def self.apply(content)
///     filter = new(content)          # `self` is the SUBCLASS
///     filter.applicable? ? ... : content
///   end
///   def applicable? = raise NotImplementedError
/// end
/// ```
///
/// Binding that to the owner made every subclass's `apply` build the
/// ABSTRACT BASE, and campfire wraps the chain in its own
/// `rescue Exception` returning `""` — so every message body rendered
/// EMPTY behind a 200 and a green suite. The subclass set of an
/// ingested tree is closed, so the method is COPIED into each
/// descendant with the receiver bound to that descendant.
const FILTERS: &str = r#"class Filter
  def initialize(content)
    @content = content
  end

  def self.apply(content)
    new(content).run
  end

  def run
    "base"
  end
end
"#;

const UPCASE: &str = r#"class Upcase < Filter
  def run
    "upcase"
  end
end
"#;

fn filter_tree() -> Vec<(String, String)> {
    emitted_files(&[
        ("app/models/filter.rb", FILTERS),
        ("app/models/upcase.rb", UPCASE),
    ])
}

fn file_named(files: &[(String, String)], suffix: &str) -> String {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| panic!("no {suffix} in {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>()))
}

#[test]
fn an_inherited_class_side_constructor_is_copied_into_the_subclass() {
    let files = filter_tree();
    let sub = file_named(&files, "upcase.rb");
    assert!(
        sub.contains("def self.apply"),
        "the subclass needs its own copy — inheriting the base's would \
         build a base:\n{sub}"
    );
    assert!(
        sub.contains("Upcase.new(content)"),
        "the copy's constructor must name the class it now lives in:\n{sub}"
    );
}

/// The base keeps its own, bound to itself: `Filter.apply` in Ruby
/// builds a `Filter`.
#[test]
fn the_base_still_constructs_itself() {
    let base = file_named(&filter_tree(), "filter.rb");
    assert!(
        base.contains("Filter.new(content)"),
        "the base's own copy is unchanged:\n{base}"
    );
}

/// A subclass that DEFINES the name shadows the inherited one, and
/// everything below it inherits the shadow — so no copy goes there.
#[test]
fn a_subclass_that_defines_the_name_is_left_alone() {
    let files = emitted_files(&[
        ("app/models/filter.rb", FILTERS),
        (
            "app/models/upcase.rb",
            "class Upcase < Filter\n  def self.apply(content)\n    \"own\"\n  end\nend\n",
        ),
    ]);
    let sub = file_named(&files, "upcase.rb");
    assert!(
        sub.contains("\"own\""),
        "the subclass's own definition must survive:\n{sub}"
    );
    assert!(
        !sub.contains("Upcase.new(content)"),
        "a class that defines the name gets no copy of the inherited one:\n{sub}"
    );
}

/// A class NOTHING subclasses keeps the owner constant — the shape the
/// rest of this file asserts, and the one every target already
/// compiles. `Sound` is that class.
#[test]
fn an_unsubclassed_class_is_not_monomorphized() {
    let src = emitted(SOUND);
    assert!(
        src.contains("Sound.new(name: \"first\")") && !src.contains("self.new"),
        "no subclass, no late binding, no copies:\n{src}"
    );
}
