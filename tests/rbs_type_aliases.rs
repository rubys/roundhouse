//! RBS `type name = ...` aliases, in `.rbs` files and in Sorbet's
//! comment form (`#: type object_type = ::User`).
//!
//! A signature that names an alias could not be read at all: the
//! reader stopped at the alias and dropped the whole method signature,
//! so the method was typed as nothing. Shopify core declares about 360
//! of them and uses them in some 940 signatures.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

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

#[test]
fn an_alias_in_an_rbs_file_stands_for_the_type_it_names() {
    let sigs = roundhouse::rbs::parse_app_signatures(
        "class Cart\n  type path = Array[Integer]\n  type keyed = Hash[Symbol, path]\n  def walk: (path, keyed) -> path\nend\n",
    )
    .expect("parse");
    let walk = &sigs[&ClassId(Symbol::new("Cart"))][&Symbol::new("walk")];
    let Ty::Fn { params, ret, .. } = walk else { panic!("{walk:?}") };
    let ints = Ty::Array { elem: std::sync::Arc::new(Ty::Int) };
    assert_eq!(params[0].ty, ints.clone().into());
    assert_eq!(
        params[1].ty,
        Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(ints.clone()) }.into(),
        "an alias may name another declared after it"
    );
    assert_eq!(**ret, ints);
}

#[test]
fn an_alias_nobody_declared_still_leaves_the_signature_unread() {
    let sigs = roundhouse::rbs::parse_signatures("class Cart\n  def walk: (nowhere) -> void\nend\n");
    assert!(sigs.is_err(), "{sigs:?}");
}

#[test]
fn a_qualified_alias_does_not_fall_back_to_a_local_bare_alias() {
    let sigs = roundhouse::rbs::parse_app_signatures(
        "class Cart\n  type path = String\n  def walk: (Route::path) -> void\nend\n",
    );
    assert!(sigs.is_err(), "{sigs:?}");
}

#[test]
fn root_qualified_unqualified_and_inherited_aliases_still_resolve() {
    let sigs = roundhouse::rbs::parse_app_signatures(
        "type path = String\nclass Cart\n  type count = Integer\n  class Nested\n    def walk: (path, ::path, count) -> void\n  end\nend\n",
    ).expect("parse");
    let Ty::Fn { params, .. } = &sigs[&ClassId(Symbol::new("Cart::Nested"))][&Symbol::new("walk")] else { panic!() };
    assert_eq!(params.iter().map(|p| (*p.ty).clone()).collect::<Vec<_>>(), vec![Ty::Str, Ty::Str, Ty::Int]);
}

const SCHEMA: (&str, &str) = ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n");

/// The comment form, as core writes it: the alias in the class body
/// (one of them continued across `#|` lines), used by a signature
/// below and by a method in a class nested inside.
#[test]
fn a_comment_alias_types_the_parameters_of_the_signatures_that_use_it() {
    let failures = dispatch_failures(&[
        SCHEMA,
        ("app/models/line.rb", "class Line\n  def cents\n    1\n  end\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/cart_controller.rb",
            r#"class CartController < ApplicationController
  #: type lines = Array[Line]
  #: type by_key = Hash[
  #|   Symbol,
  #|   Line
  #| ]

  #: (lines, by_key) -> Integer
  def totals(lines, by_key)
    lines.each { |l| l.cents.bogus_from_alias }
    by_key.each { |_k, l| l.cents.bogus_from_multiline_alias }
    1
  end

  #: (lines, by_key) -> void
  def use(lines, by_key)
    totals(lines, by_key).bogus_from_return
  end
end
"#,
        ),
    ]);
    assert_eq!(
        failures,
        vec![
            "bogus_from_alias".to_string(),
            "bogus_from_multiline_alias".to_string(),
            "bogus_from_return".to_string(),
        ]
    );
}

#[test]
fn inherited_comment_aliases_keep_their_definition_scope() {
    for declarations in [
        "#: type item = Integer\n    #: type local_items = Array[item]",
        "#: type local_items = Array[item]\n    #: type item = Integer",
    ] {
        let source = format!(r#"class Outer
  #: type item = String
  #: type items = Array[item]
  class Inner
    {declarations}
    #: type unused = (
    #: (items, local_items, item) -> void
    def consume(outer_values, inner_values, value)
      nil
    end
  end
end
"#);
        let signatures = roundhouse::ingest::sorbet_sig::ingest_sorbet_signatures(source.as_bytes());
        let consume = &signatures[&ClassId(Symbol::new("Outer::Inner"))][&Symbol::new("consume")];
        let Ty::Fn { params, .. } = consume else { panic!("{consume:?}") };
        assert_eq!(params[0].ty, Ty::Array { elem: std::sync::Arc::new(Ty::Str) }.into(), "{source}");
        assert_eq!(params[1].ty, Ty::Array { elem: std::sync::Arc::new(Ty::Int) }.into(), "{source}");
        assert_eq!(params[2].ty, Ty::Int.into(), "{source}");
    }
}

#[test]
fn an_unread_local_alias_does_not_fall_back_to_the_outer_alias() {
    let source = b"class Outer\n  #: type item = String\n  class Inner\n    #: type item = missing\n    #: (item) -> void\n    def consume(value); nil; end\n  end\nend\n";
    let signatures = roundhouse::ingest::sorbet_sig::ingest_sorbet_signatures(source);
    assert!(!signatures.contains_key(&ClassId(Symbol::new("Outer::Inner"))), "{signatures:?}");
}
