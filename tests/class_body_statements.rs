//! A class body's statements run while the file is being required,
//! and the emit has to treat them that way.
//!
//! Two halves, both found by BOOTING an emitted tree rather than by
//! checking it.
//!
//! A class-body CALL reads its arguments' constants at require time,
//! exactly as a constant initializer does — and got no require for
//! them, so the tree stopped at an `uninitialized constant` the source
//! app never sees because Rails autoloads.
//!
//! And `self.x = y` in a class body was not captured at all: the whole
//! class-body branch requires a receiverless call, so an attribute
//! write on `self` fell out without even the diagnostic a dropped
//! receiverless call gets.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(files: &[(&str, &str)], want: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    for (p, c) in files {
        tree.insert(PathBuf::from(*p), c.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with(want))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("{want} is emitted"))
}

#[test]
fn a_class_body_call_requires_the_constants_it_names() {
    // `handles(Failures::NotFound, …)` runs as the file loads. Without
    // a require for it, the emitted tree raises `uninitialized
    // constant` there — which checking never shows, because the
    // reference itself resolves fine.
    let emitted = emitted(
        &[
            ("app/services/not_found.rb", "class NotFound\nend\n"),
            (
                "app/services/resolver.rb",
                "class Resolver < Vendor::Base\n  handles(NotFound, with: \"nope\")\nend\n",
            ),
        ],
        "resolver.rb",
    );
    assert!(
        emitted.contains("require_relative") && emitted.contains("not_found"),
        "the call's constant needs a require; got:\n{emitted}"
    );
}

#[test]
fn a_method_body_call_does_not() {
    // The other half of the existing rule, and the reason this is not
    // simply "require everything": a method body runs at request time,
    // after boot, and requiring its targets closed a cycle that broke
    // a whole app's boot once already.
    let emitted = emitted(
        &[
            ("app/services/not_found.rb", "class NotFound\nend\n"),
            (
                "app/services/resolver.rb",
                "class Resolver < Vendor::Base\n  def run\n    NotFound\n  end\nend\n",
            ),
        ],
        "resolver.rb",
    );
    assert!(
        !emitted.contains("not_found"),
        "a method-body reference stays unrequired; got:\n{emitted}"
    );
}

#[test]
fn a_self_attribute_write_survives_where_the_class_body_is_replayed() {
    // A gem whose DSL is the class body: the base is one roundhouse
    // does not model, so the body is replayed verbatim. `self.x = y`
    // was falling out of that replay.
    let emitted = emitted(
        &[
            ("app/services/inner.rb", "class Inner\nend\n"),
            (
                "app/services/widget.rb",
                "class Widget < Vendor::Base\n  step :go\n  self.default_thing = Inner\nend\n",
            ),
        ],
        "widget.rb",
    );
    assert!(emitted.contains("self.default_thing = Inner"), "got:\n{emitted}");
    // The receiverless call beside it kept working.
    assert!(emitted.contains("step :go"), "got:\n{emitted}");
}

#[test]
fn a_class_whose_base_is_modelled_still_drops_its_body() {
    // The guard: the replay rule is about a base roundhouse does NOT
    // model. A plain class must not start replaying DSL it never
    // replayed, `self.`-receiver or otherwise.
    let emitted = emitted(
        &[(
            "app/services/widget.rb",
            "class Widget\n  self.default_thing = 1\n\n  def go\n    2\n  end\nend\n",
        )],
        "widget.rb",
    );
    assert!(!emitted.contains("default_thing"), "got:\n{emitted}");
    assert!(emitted.contains("def go"), "got:\n{emitted}");
}
