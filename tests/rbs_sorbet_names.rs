//! Sorbet's type names inside RBS comments: `T::Array[Line]`,
//! `T::Hash[Symbol, Line]`, `T::Boolean`.
//!
//! Sorbet's RBS-comment reader accepts them next to plain RBS, and
//! Shopify core writes about 2,800 of them (`#: (T::Array[Foo]) ->
//! T::Boolean`). The RBS reader took `T::Array` for a class of that
//! name: a list with no `each` and a flag that is an object nobody
//! defines. Dispatch on an unknown class answers untyped instead of
//! failing, so nothing complained — every block parameter and every
//! read of the result below the signature just quietly lost its type.
//!
//! The tests observe the type through the one thing a real type does
//! that an unknown class does not: a call to a method it lacks fails.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

fn dispatch_failures(files: &[(&str, &str)]) -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let mut out: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => Some(method.as_str().to_string()),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

const SCHEMA: (&str, &str) = ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n");
const LINE: (&str, &str) =
    ("app/models/line.rb", "class Line\n  def cents\n    1\n  end\nend\n");

#[test]
fn sorbet_generics_and_boolean_in_a_comment_signature_are_the_types_they_name() {
    let failures = dispatch_failures(&[
        SCHEMA,
        LINE,
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/cart_controller.rb",
            r#"class CartController < ApplicationController
  #: (T::Array[Line], T::Hash[Symbol, Line], T::Boolean) -> T::Array[Integer]
  def totals(lines, by_key, flag)
    lines.each { |l| l.cents.bogus_from_array }
    by_key.each { |_k, l| l.cents.bogus_from_hash }
    flag.bogus_from_boolean
    []
  end

  #: (T::Array[Line], T::Hash[Symbol, Line], T::Boolean) -> void
  def use(lines, by_key, flag)
    totals(lines, by_key, flag).size.bogus_from_return
  end
end
"#,
        ),
    ]);
    assert_eq!(
        failures,
        vec![
            "bogus_from_array".to_string(),
            "bogus_from_boolean".to_string(),
            "bogus_from_hash".to_string(),
            "bogus_from_return".to_string(),
        ]
    );
}

#[test]
fn sorbet_names_in_a_sidecar_rbs_file_read_the_same_way() {
    let sigs = roundhouse::rbs::parse_app_signatures(
        "class Cart\n  def flags: (T::Array[Integer]) -> T::Boolean\nend\n",
    )
    .expect("parse");
    let flags = &sigs[&roundhouse::ident::ClassId(roundhouse::ident::Symbol::new("Cart"))]
        [&roundhouse::ident::Symbol::new("flags")];
    let roundhouse::ty::Ty::Fn { params, ret, .. } = flags else { panic!("{flags:?}") };
    assert_eq!(params[0].ty, roundhouse::ty::Ty::Array { elem: Box::new(roundhouse::ty::Ty::Int) });
    assert_eq!(**ret, roundhouse::ty::Ty::Bool);
}
