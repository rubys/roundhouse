//! The two sorbet constructs left in the emit, which are not the same
//! kind of thing.
//!
//! `NAME = T.type_alias { … }` is a TYPE. It generates nothing and
//! answers nothing — the only thing that reads it is a `sig`, and
//! those are read before they are dropped — so the constant goes with
//! them.
//!
//! `T.absurd(x)` is BEHAVIOR. It sits in the branch a case analysis is
//! supposed to have made unreachable, and it raises there. Dropping it
//! would turn "this cannot happen" into "this returns nil", in the one
//! branch where a wrong answer travels furthest before anything
//! notices, so it lowers to the raise sorbet performs.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(source: &str, file: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/services/gauge.rb", source),
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
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with(file))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("{file} is emitted"))
}

#[test]
fn a_type_alias_constant_goes_with_the_sigs_that_read_it() {
    let emitted = emitted(
        r#"class Gauge
  Reading = T.type_alias { T.any(Integer, Float) }
  LIMIT = 100

  def over?(value)
    value > LIMIT
  end
end
"#,
        "gauge.rb",
    );
    assert!(!emitted.contains("type_alias"), "got:\n{emitted}");
    assert!(!emitted.contains("Reading"), "got:\n{emitted}");
    // An ordinary constant in the same body is untouched — the rule is
    // about what the initializer IS, not about where it sits.
    assert!(emitted.contains("LIMIT = 100"), "got:\n{emitted}");
    assert!(emitted.contains("def over?"), "got:\n{emitted}");
}

#[test]
fn a_type_alias_in_a_controller_goes_too() {
    // A controller body has its own ingest path, and it carried the
    // alias when the class-body one had stopped — the emitted
    // controller then died at load on a constant initializer.
    let tree: HashMap<PathBuf, Vec<u8>> = [
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
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  Reading = T.type_alias { T.any(Integer, Float) }\n  LIMIT = 100\n\n  def index\n    render plain: LIMIT.to_s\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let emitted = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("gauges_controller.rb"))
        .map(|f| f.content)
        .expect("the controller is emitted");
    assert!(!emitted.contains("type_alias"), "got:\n{emitted}");
    assert!(emitted.contains("LIMIT = 100"), "got:\n{emitted}");
}

#[test]
fn absurd_raises_where_it_stood() {
    let emitted = emitted(
        r#"class Gauge
  def label(kind)
    if kind == :low
      "low"
    else
      T.absurd(kind)
    end
  end
end
"#,
        "gauge.rb",
    );
    // Dropped, this branch would answer nil instead of raising. (The
    // CALL is gone; the name survives inside sorbet's own message.)
    assert!(!emitted.contains("T.absurd(kind)"), "got:\n{emitted}");
    assert!(emitted.contains("raise"), "got:\n{emitted}");
    assert!(emitted.contains("TypeError"), "got:\n{emitted}");
    // sorbet's own message, with the value that got there — a bare
    // "unreachable" says nothing at the point it is reached.
    assert!(
        emitted.contains("Control flow reached T.absurd. Got value: "),
        "got:\n{emitted}"
    );
    assert!(emitted.contains("kind.to_s"), "got:\n{emitted}");
}

#[test]
fn an_assertion_still_unwraps_rather_than_raising() {
    // `T.absurd` is the exception among the `T.` calls, not the rule:
    // the assertions still evaluate to their argument.
    let emitted = emitted(
        r#"class Gauge
  def label(kind)
    T.must(kind).to_s
  end
end
"#,
        "gauge.rb",
    );
    assert!(!emitted.contains("raise"), "got:\n{emitted}");
    assert!(emitted.contains("kind.to_s"), "got:\n{emitted}");
}
