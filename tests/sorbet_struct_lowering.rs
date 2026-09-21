//! A `T::Struct` is lowered into the plain Ruby it stands for.
//!
//! `const :name, String` is not an annotation: it declares a reader
//! and one keyword of the constructor sorbet generates. Carrying it
//! into the emit would need sorbet-runtime at load time to build a
//! single value object; dropping it would leave a class that cannot be
//! built at all. So it is lowered — the declarations become the
//! methods they stand for, and the superclass goes with them.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(source: &str, file: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"sightings\", force: :cascade do |t|\n    t.string \"target\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/services/observation.rb", source),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/sightings\", to: \"sightings#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with(file))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("{file} is emitted"))
}

#[test]
fn a_const_becomes_a_reader_and_a_keyword_of_the_constructor() {
    let emitted = emitted(
        r#"module Skywatch
  class Observation < T::Struct
    const :target, String
    const :magnitude, Float
  end
end
"#,
        "observation.rb",
    );
    // The declaration is gone, and so is the class generator it spoke
    // to: neither has a runtime in the emitted tree.
    assert!(!emitted.contains("const :"), "got:\n{emitted}");
    assert!(!emitted.contains("T::Struct"), "got:\n{emitted}");
    // What it declared is there instead.
    assert!(emitted.contains("def target"), "got:\n{emitted}");
    assert!(emitted.contains("def magnitude"), "got:\n{emitted}");
    // Keyword, not positional — that is the constructor sorbet builds,
    // so it is the one every call site already passes.
    assert!(
        emitted.contains("def initialize(target:, magnitude:)"),
        "got:\n{emitted}"
    );
    assert!(emitted.contains("@target = target"), "got:\n{emitted}");
    // A `const` is read-only.
    assert!(!emitted.contains("def target="), "got:\n{emitted}");
}

#[test]
fn a_prop_is_writable_where_a_const_is_not() {
    let emitted = emitted(
        r#"class Sighting < T::Struct
  const :target, String
  prop :confirmed, T::Boolean
end
"#,
        "sighting.rb",
    );
    assert!(emitted.contains("def confirmed="), "got:\n{emitted}");
    assert!(!emitted.contains("def target="), "got:\n{emitted}");
}

#[test]
fn a_default_and_a_factory_reach_the_constructor() {
    let emitted = emitted(
        r#"class Reading < T::Struct
  const :target, String
  const :magnitude, Float, default: 0.0
  const :notes, T::Array[String], factory: -> { [] }
end
"#,
        "reading.rb",
    );
    // Without the default the keyword would be REQUIRED, and every
    // call site that omits it would raise instead of taking it.
    assert!(
        emitted.contains("magnitude: 0.0"),
        "the default reaches the constructor; got:\n{emitted}"
    );
    // A factory is evaluated per instance, which is what a default
    // EXPRESSION in a keyword is — a shared literal would be one array
    // for every instance of the class.
    assert!(
        emitted.contains("notes: []"),
        "the factory body reaches the constructor; got:\n{emitted}"
    );
}

#[test]
fn acts_as_comparable_keeps_its_value_equality() {
    let source = r#"class Fix < T::Struct
  include T::Struct::ActsAsComparable

  const :target, String
end
"#;
    let comparable = emitted(source, "fix.rb");
    assert!(!comparable.contains("ActsAsComparable"), "got:\n{comparable}");
    // Dropping the mixin without this would put object identity in the
    // place of value equality — the same expression answering
    // differently, and nothing to see at the call site.
    assert!(comparable.contains("def =="), "got:\n{comparable}");
    assert!(
        comparable.contains("@target == other.target"),
        "got:\n{comparable}"
    );

    // A struct without the mixin does not get one: `==` is identity
    // there, and synthesizing it would be a behavior change.
    let plain = emitted(
        "class Bearing < T::Struct\n  const :target, String\nend\n",
        "bearing.rb",
    );
    assert!(!plain.contains("def =="), "got:\n{plain}");
}

/// The same lowering under a base that is NOT `T::Struct`.
///
/// A gem's base class is opaque to the tree: `const :amount, Integer`
/// in its subclass is the same `T::Props` macro, and the only way to
/// know that is the base's ancestry. Guessing from the method name is
/// what `docs/guide/transpile.md` says not to do — so a sidecar
/// states it, and the expansion follows.
fn emitted_with_sidecar(source: &str, rbs: &str, file: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"sightings\", force: :cascade do |t|\n    t.string \"target\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/services/observation.rb", source),
        ("sig/vendor.rbs", rbs),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/sightings\", to: \"sightings#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with(file))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("{file} is emitted"))
}

const VENDOR_DIRECT: &str = "class Vendor::Operation\n  include T::Props\nend\n";

#[test]
fn a_gem_base_the_sidecar_says_has_props_gets_the_expansion() {
    let emitted = emitted_with_sidecar(
        r#"class Observation < Vendor::Operation
  const :amount, Integer
  const :label, String
end
"#,
        VENDOR_DIRECT,
        "observation.rb",
    );
    // The reader the macro stands for, and the keyword constructor.
    assert!(emitted.contains("def amount"), "got:\n{emitted}");
    assert!(emitted.contains("def label"), "got:\n{emitted}");
    assert!(emitted.contains("def initialize("), "got:\n{emitted}");
    // And the declaration itself is gone — expanded, not replayed.
    assert!(!emitted.contains("const :amount"), "got:\n{emitted}");
}

#[test]
fn the_ancestry_is_followed_through_an_intermediate_module() {
    // The shape that actually occurs: the base includes a module that
    // includes `T::Props`. Flattening that at the sidecar would be the
    // app asserting something it did not write.
    let emitted = emitted_with_sidecar(
        r#"class Observation < Vendor::Operation
  const :amount, Integer
end
"#,
        "class Vendor::Operation\n  include Vendor::TypedProps\nend\n\nmodule Vendor::TypedProps\n  include T::Props\nend\n",
        "observation.rb",
    );
    assert!(emitted.contains("def amount"), "got:\n{emitted}");
    assert!(!emitted.contains("const :amount"), "got:\n{emitted}");
}

#[test]
fn a_gem_base_without_props_still_replays_its_body() {
    // The guard, and the reason the sidecar is consulted rather than
    // the method name: `const` under a base that does NOT include
    // `T::Props` is somebody else's DSL, and replaying it is right.
    let emitted = emitted_with_sidecar(
        r#"class Observation < Vendor::Operation
  const :amount, Integer
end
"#,
        "class Vendor::Operation\nend\n",
        "observation.rb",
    );
    assert!(emitted.contains("const :amount"), "got:\n{emitted}");
    assert!(!emitted.contains("def amount"), "got:\n{emitted}");
}

#[test]
fn a_class_that_includes_the_module_itself_gets_the_expansion() {
    // The ancestry can arrive two ways. A base without `T::Props`
    // whose SUBCLASS includes the module in its own body reaches it
    // just the same, and its `const` is the same macro.
    //
    // Found by claiming the opposite: a sidecar saying the BASE had
    // the module produced the right emit for the wrong reason, and a
    // boot census contradicted the claim.
    let emitted = emitted_with_sidecar(
        r#"class Observation < Vendor::Plain
  include Vendor::TypedProps

  const :amount, Integer
end
"#,
        "class Vendor::Plain\nend\n\nmodule Vendor::TypedProps\n  include T::Props\nend\n",
        "observation.rb",
    );
    assert!(emitted.contains("def amount"), "got:\n{emitted}");
    assert!(!emitted.contains("const :amount"), "got:\n{emitted}");
}
