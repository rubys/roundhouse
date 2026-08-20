//! An STI SUBCLASS standing at a query root.
//!
//! `Rooms::Open.count` counts the rooms whose `type` column says
//! `"Rooms::Open"` — Rails gives every subclass its own default scope.
//! An STI subclass declares no table, so it is not ingested as a Model,
//! and every one of those call sites resolved by plain Ruby inheritance
//! to the BASE class's method and answered the whole table. campfire has
//! seven rooms across three subclasses; `Rooms::Open.count` answered 7
//! where Rails answers 2.
//!
//! The rewrite is a call-site one, into vocabulary every target already
//! speaks — `Room.where(type: "Rooms::Open")` — rather than a
//! per-subclass copy of the Relation API on every target.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::app::App;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_sti_scope_lowering;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app(body: &str) -> App {
    let model = format!(
        "class Room < ApplicationRecord\n  def self.probe\n    {body}\n  end\nend\n"
    );
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
    t.string "type", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        ("app/models/room.rb", Box::leak(model.into_boxed_str())),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        (
            "app/models/rooms/open.rb",
            "class Rooms::Open < Room\n  def hi\n    1\n  end\nend\n",
        ),
        (
            "app/models/gadget.rb",
            "class Gadget < Widget\n  def hi\n    1\n  end\nend\n",
        ),
    ]))
    .expect("ingest sti app");
    Analyzer::new(&app).analyze(&mut app);
    apply_sti_scope_lowering(&mut app);
    app
}

fn probe_body(body: &str) -> String {
    let app = app(body);
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

/// A terminal on the subclass rides a type-scoped relation on the base.
#[test]
fn a_subclass_terminal_scopes_by_type() {
    let body = probe_body("Rooms::Open.pluck(:id)");
    assert!(body.contains("Room"), "rooted on the base model: {body}");
    assert!(body.contains("Rooms::Open"), "with the type as a value: {body}");
    assert!(body.contains("pluck"), "and the terminal survives: {body}");
}

/// `.all` IS the scope, so the call disappears into it.
#[test]
fn all_becomes_the_scope_itself() {
    let body = probe_body("Rooms::Open.all");
    assert!(body.contains("where"), "{body}");
    assert!(!body.contains("\"all\""), "no leftover .all: {body}");
}

/// Construction STAMPS instead of filtering. Without it
/// `Rooms::Open.create!` wrote a row with an empty `type`, belonging to
/// no subclass — which the read half above then correctly could not
/// see, so the two halves have to land together.
#[test]
fn construction_stamps_the_type_instead_of_filtering() {
    let body = probe_body("Rooms::Open.create!(name: \"x\")");
    assert!(!body.contains("where"), "a constructor is not scoped: {body}");
    assert!(
        body.contains("Str { value: \"Rooms::Open\" }"),
        "the type column must be stamped: {body}"
    );
    // The RECEIVER stays the subclass: Ruby's inheritance constructs a
    // `Rooms::Open`, which is what an `is_a?` predicate asks about.
    assert!(
        body.contains("Symbol(\"Rooms\")"),
        "the receiver stays the subclass: {body}"
    );
}

/// An explicit `type:` wins — nothing is overwritten.
#[test]
fn an_explicit_type_is_not_overwritten() {
    let body = probe_body("Rooms::Open.new(type: \"Rooms::Closed\")");
    assert!(
        !body.contains("Str { value: \"Rooms::Open\" }"),
        "an explicit type must win: {body}"
    );
}

/// Plain Ruby inheritance is not STI. `Gadget < Widget` with no `type`
/// column on `widgets` has nothing to filter on.
#[test]
fn a_base_without_a_type_column_is_not_sti() {
    let body = probe_body("Gadget.pluck(:id)");
    assert!(!body.contains("where"), "not an STI base: {body}");
}
