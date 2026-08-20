//! `type_previously_changed?(to: "Rooms::Open")`.
//!
//! Rails' Dirty predicates take optional `from:`/`to:` bounds:
//! "changed, AND the value it changed from/to was this". The
//! synthesized predicates are ZERO-ARITY, and stay that way — a
//! signature is a promise every caller pays for, and Rust and Go have
//! no default arguments, so an optional kwarg on
//! `<col>_previously_changed?` widens the shape for every model of
//! every app to serve a handful of sites. Same argument
//! `lower::route_format_suffix` makes about `format:`.
//!
//! So the bound moves to the CALL SITE, composed from the reader and
//! the value-half the same synthesis already emits.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::App;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn probe(body: &str) -> String {
    let model = format!(
        "class Room < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"
    );
    let mut app: App = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"rooms\", force: :cascade do |t|\n    t.string \"type\", null: false\n  end\nend\n",
        ),
        ("app/models/room.rb", Box::leak(model.into_boxed_str())),
    ]))
    .expect("ingest dirty-bound app");
    roundhouse::session::analyze_and_lower(&mut app);
    let room = app.models.iter().find(|m| m.name.0.as_str() == "Room").expect("Room");
    for item in &room.body {
        if let roundhouse::dialect::ModelBodyItem::Method { method, .. } = item {
            if method.name.as_str() == "probe" {
                return format!("{:?}", method.body);
            }
        }
    }
    panic!("no probe method");
}

/// `to:` becomes a comparison against the CURRENT value.
#[test]
fn a_to_bound_compares_the_current_value() {
    let ir = probe("type_previously_changed?(to: \"Rooms::Open\")");
    assert!(ir.contains("BoolOp"), "the bound is a conjunct: {ir}");
    assert!(
        ir.contains("Symbol(\"type_previously_changed?\")"),
        "the predicate survives: {ir}"
    );
    assert!(ir.contains("Symbol(\"==\")"), "and a comparison joins it: {ir}");
    // Zero-arity: nothing is passed to the predicate itself.
    assert!(
        !ir.contains("Symbol(\"type_previously_changed?\"), args: [Expr"),
        "the predicate stays zero-arity: {ir}"
    );
}

/// `from:` compares against the value-half the same synthesis emits.
#[test]
fn a_from_bound_compares_the_previous_value() {
    let ir = probe("type_previously_changed?(from: \"Rooms::Closed\")");
    assert!(
        ir.contains("Symbol(\"type_previously_was\")"),
        "the `from` bound reads the previous value: {ir}"
    );
}

/// An option this does not understand is left ALONE rather than
/// silently dropped.
#[test]
fn an_unrecognized_option_is_left_alone() {
    let ir = probe("type_previously_changed?(mystery: 1)");
    assert!(!ir.contains("BoolOp"), "no rewrite: {ir}");
}
