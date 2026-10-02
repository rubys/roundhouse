//! A constant read in the class that declares it types as its value.
//!
//! `PREFIX = "main"` and a `def direct; PREFIX; end` beside it: the
//! read is `String`, not a `Ty::Class` named `PREFIX` — a class nothing
//! defines, which reached the emitted RBS as `() -> PREFIX`. Library
//! classes were typed with an EMPTY constant environment, and the
//! global registry drops a name two classes both declare, which is the
//! common case for a per-component prefix or default. The read is
//! lexically inside the declaring class, so the owner's own table
//! answers first (#130).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn app_with(files: &[(&str, &str)]) -> roundhouse::App {
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
        tree.insert(PathBuf::from(p), c.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn library_ret(app: &roundhouse::App, class: &str, method: &str) -> Ty {
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name == ClassId(Symbol::new(class)))
        .unwrap_or_else(|| panic!("{class} is a library class"));
    let m = lc
        .methods
        .iter()
        .find(|m| m.name == Symbol::new(method))
        .unwrap_or_else(|| panic!("{class}#{method} exists"));
    match m.signature.clone().expect("inference wrote a signature") {
        Ty::Fn { ret, .. } => *ret,
        other => panic!("not a Fn: {other:?}"),
    }
}

const LABELLER: &str = r#"class Labeller
  PREFIX = "main"

  def direct
    PREFIX
  end
end
"#;

#[test]
fn a_library_class_reads_its_own_constant_as_its_value() {
    let app = app_with(&[("app/services/labeller.rb", LABELLER)]);
    assert_eq!(library_ret(&app, "Labeller", "direct"), Ty::Str);
}

#[test]
fn a_name_two_classes_declare_still_resolves_inside_each() {
    // The shape that found this: one component class per file, each
    // declaring the same constant name with a different value. The
    // global by-name registry rightly gives up on the bare name; the
    // read inside each declaring class must not.
    let app = app_with(&[
        ("app/services/labeller.rb", LABELLER),
        (
            "app/services/counter.rb",
            "class Counter\n  PREFIX = 7\n\n  def direct\n    PREFIX\n  end\nend\n",
        ),
    ]);
    assert_eq!(library_ret(&app, "Labeller", "direct"), Ty::Str);
    assert_eq!(library_ret(&app, "Counter", "direct"), Ty::Int);
}

#[test]
fn a_model_reads_its_own_constant_as_its_value() {
    let app = app_with(&[
        (
            "app/models/gauge.rb",
            "class Gauge < ApplicationRecord\n  PREFIX = \"main\"\n\n  def direct\n    PREFIX\n  end\nend\n",
        ),
        (
            "app/models/labeller.rb",
            "class Labeller\n  PREFIX = 7\n\n  def direct\n    PREFIX\n  end\nend\n",
        ),
    ]);
    let model = app
        .models
        .iter()
        .find(|m| m.name == ClassId(Symbol::new("Gauge")))
        .expect("Gauge is a model");
    let m = model
        .methods()
        .find(|m| m.name == Symbol::new("direct"))
        .expect("Gauge#direct");
    assert_eq!(m.body.ty, Some(Ty::Str), "got {:?}", m.body.ty);
    assert_eq!(library_ret(&app, "Labeller", "direct"), Ty::Int);
}

#[test]
fn runtime_values_keep_their_owner_and_survive_a_source_roundtrip() {
    let mut app = app_with(&[
        ("app/services/runtime_value_probe.rb", r#"class RuntimeValueProbe
  ATTRIBUTES = 17
  def direct
    ActionText::Attachment::ATTRIBUTES
  end
  def local
    ATTRIBUTES
  end
end
"#),
    ]);
    let expected = Ty::Array { elem: Box::new(Ty::Str) };
    assert_eq!(library_ret(&app, "RuntimeValueProbe", "direct"), expected);
    assert_eq!(library_ret(&app, "RuntimeValueProbe", "local"), Ty::Int);
    app = serde_json::from_slice(&serde_json::to_vec(&app).expect("serialize")).expect("deserialize");
    roundhouse::session::analyze_and_lower(&mut app);
    assert_eq!(library_ret(&app, "RuntimeValueProbe", "direct"), expected);
}

#[test]
fn an_app_runtime_constant_override_cannot_borrow_the_old_literal_type() {
    let app = app_with(&[
        ("app/services/attachment.rb", "class ActionText::Attachment\n  ATTRIBUTES = 17\nend\n"),
        ("app/services/runtime_value_probe.rb", "class RuntimeValueProbe\n  def direct\n    ActionText::Attachment::ATTRIBUTES\n  end\nend\n"),
    ]);
    assert_eq!(library_ret(&app, "RuntimeValueProbe", "direct"), Ty::Int);
}

#[test]
fn an_unresolved_app_override_does_not_regain_the_runtime_array_type() {
    let app = app_with(&[
        ("app/services/attachment.rb", "class ActionText::Attachment\n  ATTRIBUTES = MissingRuntimeBuilder.build\nend\n"),
        ("app/controllers/gauges_controller.rb", "class GaugesController < ApplicationController\n  def index\n    ActionText::Attachment::ATTRIBUTES\n  end\nend\n"),
    ]);
    let probe = app.controllers.iter().find(|class| class.name.0.as_str() == "GaugesController").unwrap();
    let method = probe.actions().find(|method| method.name.as_str() == "index").unwrap();
    assert!(matches!(method.body.ty, Some(Ty::Var { .. })), "{:?}", method.body.ty);
    let diags = roundhouse::analyze::diagnose(&app);
    assert!(diags.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error
        && d.message.contains("ActionText::Attachment::ATTRIBUTES")), "{diags:?}");
}
