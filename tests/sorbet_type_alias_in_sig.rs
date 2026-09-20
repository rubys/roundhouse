//! A `sig` that names a type alias gets the type, not a phantom class.
//!
//! `Name = T.type_alias { … }` is the one constant a `sig` can name
//! that is NOT a class, and the reader could not tell the difference:
//! `params(data: ResultData)` became `Ty::Class { "ResultData" }`, a
//! class nothing defines. Harmless while the declared parameter type
//! never reached the body — and an error the moment it did.
//!
//! The emit already drops these constants for having no runtime (they
//! are types, and the signatures that read them are gone by then).
//! This is the other half: reading what they were for.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn app_with(service: &str) -> roundhouse::App {
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
        ("app/services/reporter.rb", service),
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
    app
}

fn param_ty(app: &roundhouse::App, class: &str, method: &str) -> Option<Ty> {
    let ty = app
        .rbs_signatures
        .get(&ClassId(Symbol::new(class)))
        .and_then(|m| m.get(&Symbol::new(method)))
        .cloned()?;
    match ty {
        Ty::Fn { params, .. } => params.first().map(|p| p.ty.clone()),
        _ => None,
    }
}

#[test]
fn an_alias_resolves_to_the_type_it_names() {
    let app = app_with(
        r#"class Reporter
  Label = T.type_alias { String }

  sig { params(label: Label).void }
  def record(label)
    @label = label
  end
end
"#,
    );
    assert_eq!(
        param_ty(&app, "Reporter", "record"),
        Some(Ty::Str),
        "the alias names String, so the parameter is String"
    );
}

#[test]
fn a_union_alias_keeps_its_variants() {
    // The shape that found this: an alias over `T.any(…)` of two
    // qualified classes, which is exactly what a bare `Ty::Class` for
    // the alias name cannot stand in for.
    let app = app_with(
        r#"class Reporter
  Source = T.type_alias { T.any(String, Integer) }

  sig { params(source: Source).void }
  def record(source)
    @source = source
  end
end
"#,
    );
    let ty = param_ty(&app, "Reporter", "record").expect("the sig is read");
    assert_eq!(ty, Ty::Union { variants: vec![Ty::Str, Ty::Int] }, "got {ty:?}");
}

#[test]
fn the_order_of_alias_and_sig_does_not_matter() {
    // Collected before the body is walked, so a `sig` above the alias
    // reads it too. Ruby would not run that way, but a reader that
    // depended on statement order would be a trap nobody can see.
    let app = app_with(
        r#"class Reporter
  sig { params(label: Label).void }
  def record(label)
    @label = label
  end

  Label = T.type_alias { String }
end
"#,
    );
    assert_eq!(param_ty(&app, "Reporter", "record"), Some(Ty::Str));
}

#[test]
fn a_constant_that_is_a_real_class_is_untouched() {
    // A guard rather than a proof: green before this change and after
    // it. The plausible way to get this wrong is to treat every
    // constant in a class body as a candidate, and that is what this
    // would catch.
    let app = app_with(
        r#"class Reporter
  class Payload
  end

  sig { params(payload: Payload).void }
  def record(payload)
    @payload = payload
  end
end
"#,
    );
    let ty = param_ty(&app, "Reporter", "record").expect("the sig is read");
    assert!(
        matches!(&ty, Ty::Class { id, .. } if id.0.as_str().ends_with("Payload")),
        "got {ty:?}"
    );
}
