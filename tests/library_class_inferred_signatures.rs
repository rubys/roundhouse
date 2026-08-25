//! A plain library class ships the shapes the analyzer already knew.
//!
//! `stamp_inferred_library_signatures` used to be
//! `stamp_inferred_helper_signatures`, scoped to the classes named in
//! `app.helper_method_index`. Every other class — a PORO in
//! `app/models`, a service object in `lib/` — emitted a sidecar that
//! was `untyped` from end to end, while `self.classes` in the analyzer
//! held `String`, `Integer`, `Array[Integer]` for the very same
//! methods. The registry and the sidecar were two descriptions of one
//! program that disagreed, and the sidecar is the one spinel reads.
//!
//! Lobsters paid for it: `ShortId::CandidateId#to_s` registered as
//! `String` and shipped as `() -> untyped`, a `def to_s: () -> untyped`
//! in the program widened every polymorphic `.to_s`, and a
//! `Hash[String, String]` came back poly far enough downstream to
//! produce two C errors in an otherwise-clean AOT build.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

fn sidecars(lib_src: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/probe.rb"), lib_src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

const LITERALS: &str = r#"
class Probe
  def lit
    "abc"
  end

  def chained
    "abc".downcase
  end

  def num
    42
  end

  def arr
    [1, 2]
  end
end
"#;

#[test]
fn a_plain_class_is_not_untyped_end_to_end() {
    let rbs = sidecars(LITERALS);
    assert!(rbs.contains("def lit: () -> String"), "{rbs}");
    assert!(rbs.contains("def chained: () -> String"), "{rbs}");
    assert!(rbs.contains("def num: () -> Integer"), "{rbs}");
    assert!(rbs.contains("def arr: () -> Array[Integer]"), "{rbs}");
}

// The lobsters chain, in miniature: a class method whose return feeds
// an accessor, whose reader feeds `to_s`. Every link needs the one
// before it, so this is also the fixpoint-depth test.
const CHAIN: &str = r#"
class Ident
  attr_accessor :id

  def initialize
    self.id = generate_id
  end

  def to_s
    id
  end

  def generate_id
    Maker.build(6).downcase
  end
end

class Maker
  def self.build(len)
    str = ""
    while str.length < len
      str += "x"
    end
    return str
  end
end
"#;

#[test]
fn a_return_travels_the_whole_chain() {
    let rbs = sidecars(CHAIN);
    assert!(rbs.contains("def self.build: (Integer len) -> String"), "{rbs}");
    assert!(rbs.contains("def generate_id: () -> String"), "{rbs}");
    // `@id` is genuinely unset until `initialize` runs, so `String?` is
    // the honest answer — the point is that `String` survives at all.
    assert!(rbs.contains("def to_s: () -> String?"), "{rbs}");
    assert!(rbs.contains("attr_reader id: String?"), "{rbs}");
    assert!(rbs.contains("attr_writer id: String"), "{rbs}");
}

#[test]
fn initialize_is_left_alone() {
    // Its body type is whatever the last statement happened to be
    // (`self.id = generate_id` types as `String`). `new` answers the
    // class; declaring `initialize` returns a String is a lie the
    // untyped fallback does not tell.
    let rbs = sidecars(CHAIN);
    assert!(rbs.contains("def initialize: () -> untyped"), "{rbs}");
}

// A rest slot collects every trailing argument, but `collect_send_sites`
// records argument types BY POSITION — the slot under `*streams` only
// ever sees the FIRST vararg of any call.
const REST: &str = r#"
class Silencer
  def self.silence(*streams)
    streams.length
  end

  def self.call
    silence("a", 1)
  end
end
"#;

#[test]
fn a_rest_slot_is_not_declared_from_its_first_vararg() {
    let rbs = sidecars(REST);
    assert!(rbs.contains("def self.silence: (*untyped streams) -> Integer"), "{rbs}");
}

// `String | untyped` IS `untyped`. Rendering the members alongside the
// gradual arm advertises a precision the type does not have, and spinel
// reads the result as a real union and emits poly dispatch for it.
const MIXED: &str = r#"
class Mixed
  def self.build(flag)
    if flag
      "a"
    else
      Unknowable.thing
    end
  end
end
"#;

#[test]
fn a_union_with_a_gradual_arm_renders_as_untyped() {
    let rbs = sidecars(MIXED);
    assert!(rbs.contains("-> untyped"), "{rbs}");
    assert!(!rbs.contains("String | untyped"), "{rbs}");
    assert!(!rbs.contains("untyped?"), "{rbs}");
}
