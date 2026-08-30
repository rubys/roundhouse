//! Two lowerings that stand between a class OBJECT (or an
//! ActiveSupport core ext) and a target that resolves every call
//! statically.
//!
//! Both were found by DRIVING the campfire spinel binary for
//! rubys/roundhouse#71 items 4+5, and both had already taken a server
//! down before they had a test — which is why the assertions here are
//! on the emitted TEXT rather than on the IR. A lowering can be right
//! in the IR and undone downstream; only the emitted string says which
//! (the lesson `tests/core_class_reopen_keeps_self.rs` records).

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

/// Ingest a one-file library class beside a `Room` model and return
/// the emitted Ruby for it.
fn emit_lib(src: &str, stem: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from(format!("app/models/{stem}.rb")), src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(&format!("{stem}.rb")))
        .unwrap_or_else(|| panic!("no {stem}.rb emitted"))
        .content
}

// ── `only:` is the finder ────────────────────────────────────────────

/// `GlobalID::Locator.locate(gid, only: Room)` passes a CLASS OBJECT
/// as the thing to find on. CRuby dispatches through the singleton; a
/// strict target has none, and spinel emits a call to a class method
/// `ActiveRecord::Base` never defines. The set of classes an `only:`
/// names is closed at ingest, so the call is specialized instead.
#[test]
fn a_literal_only_becomes_a_per_model_entry_point() {
    let src = emit_lib(
        "class Finder\n  def self.room_from(gid)\n    GlobalID::Locator.locate gid, only: Room\n  end\nend\n",
        "finder",
    );
    assert!(
        src.contains("GlobalID::Locator.locate_room"),
        "the literal `only: Room` should have been specialized:\n{src}"
    );
    assert!(
        !src.contains("only:"),
        "the class object should no longer be passed at all:\n{src}"
    );
}

/// A COMPUTED `only:` is left alone. It would need exactly the
/// dispatch the pass exists to remove, and a rewrite that guessed
/// would find on a class the caller did not name — so the generic
/// `locate` still receives it, which runs on the Ruby lanes and is
/// refused at compile time on a strict one. A refusal naming the real
/// construct beats a silent wrong answer.
#[test]
fn a_computed_only_is_left_for_the_generic_locate() {
    let src = emit_lib(
        "class Finder\n  def self.room_from(gid, klass)\n    GlobalID::Locator.locate gid, only: klass\n  end\nend\n",
        "finder",
    );
    assert!(
        src.contains("only:"),
        "a non-literal `only:` must reach the generic locate unchanged:\n{src}"
    );
    assert!(
        !src.contains("locate_"),
        "a non-literal `only:` must not have been specialized:\n{src}"
    );
}

// ── ActiveSupport's Array ordinals ───────────────────────────────────

/// `Array#second` is activesupport's `array/access.rb`, whose whole
/// body is `self[1]`. Un-lowered it reached the campfire spinel binary
/// as a dynamic dispatch and answered `undefined method 'second' for
/// an instance of Array` — inside a cable subscribe, on a runtime with
/// no per-request rescue, so it ended the process.
#[test]
fn array_ordinals_become_index_reads() {
    let src = emit_lib(
        "class Splitter\n  def self.suffix(name)\n    name.to_s.split(\":\", 2).second\n  end\n\n  \
         def self.pick_fourth\n    [\"a\", \"b\", \"c\", \"d\"].fourth\n  end\nend\n",
        "splitter",
    );
    assert!(
        src.contains("split(\":\", 2)[1]"),
        "`second` on a split result should be an index read:\n{src}"
    );
    assert!(!src.contains(".second"), "no `second` should survive:\n{src}");
    assert!(!src.contains(".fourth"), "no `fourth` should survive:\n{src}");
}

/// AN UNTYPED RECEIVER IS LEFT ALONE, and that is the rule rather than
/// a shortfall: the rewrite inlines `Array`'s definition, so it is only
/// sound where the type says the receiver IS an Array. The call still
/// reaches a target that cannot answer it — recorded as a gap here
/// rather than papered over with a guess about what `xs` holds.
#[test]
fn an_untyped_receiver_keeps_the_ordinal() {
    let src = emit_lib(
        "class Splitter\n  def self.pick_from(xs)\n    xs.fourth\n  end\nend\n",
        "splitter",
    );
    assert!(
        src.contains(".fourth"),
        "an untyped receiver must not be rewritten:\n{src}"
    );
}

/// The receiver has to BE an Array. `second` also exists on
/// `ActiveRecord::Relation`, where it is `offset(1).first` — a query,
/// not an index read — and the runtime answers that one with a real
/// method and a real signature.
#[test]
fn a_relation_second_is_not_an_index_read() {
    let src = emit_lib(
        "class Picker\n  def self.runner_up\n    Room.all.second\n  end\nend\n",
        "picker",
    );
    assert!(
        src.contains(".second"),
        "a Relation receiver must keep the query form:\n{src}"
    );
}

// ── conversions type through an untyped receiver ─────────────────────

/// `x.to_s` is a String whatever `x` is, and saying so is what lets a
/// chain RECOVER from a gradual receiver instead of staying gradual to
/// its end. This is what makes the ordinal rewrite above fire at all:
/// campfire's `stream_name` is an untyped class-method parameter, so
/// without this the whole chain absorbed and the Array receiver the
/// rewrite keys on never appeared.
#[test]
fn to_s_recovers_a_chain_from_an_untyped_receiver() {
    let src = emit_lib(
        "class Splitter\n  def self.suffix(name)\n    name.to_s.split(\":\", 2).second\n  end\nend\n",
        "splitter",
    );
    assert!(
        src.contains("[1]"),
        "the ordinal after `to_s` must have typed through to an Array:\n{src}"
    );
}
