//! `T.must` and `T.must_because` lower to sorbet-runtime's nil check,
//! so their value is the argument with nil ruled out. A
//! `T.must_because` whose reason is not a literal block — a forwarded
//! `&blk`, or one taking parameters — stays unsupported: its message
//! is not in hand to inline.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

fn failed_sends(body: &str) -> Vec<(String, String)> {
    let files: [(&str, &str); 5] = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"lines\", force: :cascade do |t|\n    t.integer \"qty\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/line.rb", "class Line < ApplicationRecord\n  def cents\n    1\n  end\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/cart_controller.rb", body),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let mut out: Vec<(String, String)> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                Some((method.as_str().to_string(), format!("{recv_ty:?}")))
            }
            DiagnosticKind::Unsupported { construct, detail, .. } => Some((construct.to_string(), detail)),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// The receiver type each failure names.
fn receivers(body: &str) -> Vec<String> {
    failed_sends(body).into_iter().map(|(_, r)| r).collect()
}

fn action(line: &str) -> String {
    format!(
        "class CartController < ApplicationController\n  def show\n    {line}\n  end\nend\n"
    )
}

#[test]
fn t_must_rules_nil_out() {
    let bare = receivers(&action("Line.find_by(id: 1).bogus_after"));
    assert_eq!(bare.len(), 1, "{bare:?}");
    assert!(bare[0].contains("Nil"), "{bare:?}");
    // The one failure left is `bogus_after` on a Line that cannot be nil.
    let must = receivers(&action("T.must(Line.find_by(id: 1)).bogus_after"));
    assert_eq!(must.len(), 1, "{must:?}");
    assert!(must[0].contains("Line") && !must[0].contains("Nil"), "{must:?}");
}

#[test]
fn t_must_because_rules_nil_out() {
    let must = receivers(&action(
        "T.must_because(Line.find_by(id: 1)) { \"a line\" }.bogus_after",
    ));
    // As for `T.must`: the one failure left is `bogus_after` on a Line
    // that cannot be nil, and no refusal is reported.
    assert_eq!(must.len(), 1, "{must:?}");
    assert!(must[0].contains("Line") && !must[0].contains("Nil"), "{must:?}");
}

#[test]
fn t_must_because_without_an_inlinable_reason_is_refused() {
    // A forwarded block: the message comes from calling it, which is
    // not available at the call site, so the refusal stands.
    let forwarded = receivers(
        "class CartController < ApplicationController\n  def show(&blk)\n    T.must_because(Line.find_by(id: 1), &blk).bogus_after\n  end\nend\n",
    );
    assert!(forwarded.iter().any(|d| d.contains("nil-check")), "forwarded: {forwarded:?}");
    assert!(forwarded.iter().any(|d| d.contains("Nil")), "forwarded: {forwarded:?}");

    // A reason taking parameters: sorbet's `yield` passes none, so the
    // parameter would read as nil and a body using it would not
    // survive the inline.
    let parameterized = receivers(&action(
        "T.must_because(Line.find_by(id: 1)) { |why| why.to_s }.bogus_after",
    ));
    assert!(parameterized.iter().any(|d| d.contains("nil-check")), "parameterized: {parameterized:?}");
    assert!(parameterized.iter().any(|d| d.contains("Nil")), "parameterized: {parameterized:?}");

    // `next`/`break` answer through sorbet's `yield` and are defined
    // relative to the block. Inlined into the raise they are inside no
    // block at all, and the emitted Ruby would not parse.
    for reason in ["{ next \"a line\" }", "{ break \"a line\" }"] {
        let escaping = receivers(&action(&format!(
            "T.must_because(Line.find_by(id: 1)) {reason}.bogus_after"
        )));
        assert!(escaping.iter().any(|d| d.contains("nil-check")), "{reason}: {escaping:?}");
        assert!(escaping.iter().any(|d| d.contains("Nil")), "{reason}: {escaping:?}");
    }
}

#[test]
fn t_must_because_allows_control_flow_owned_by_a_nested_block() {
    // The `next` belongs to the inner iterator, not to the reason, so
    // the inline keeps its boundary and the lowering stands.
    let nested = receivers(&action(
        "T.must_because(Line.find_by(id: 1)) { [1].map { |n| next n }.join }.bogus_after",
    ));
    assert_eq!(nested.len(), 1, "{nested:?}");
    assert!(nested[0].contains("Line") && !nested[0].contains("Nil"), "{nested:?}");
}

#[test]
fn t_let_and_t_unsafe_do_not_rule_nil_out() {
    for wrapper in ["T.let(Line.find_by(id: 1), T.nilable(Line))", "T.unsafe(Line.find_by(id: 1))"] {
        let r = receivers(&action(&format!("{wrapper}.bogus_after")));
        assert!(r.iter().all(|t| t.contains("Nil") || t.contains("Untyped")), "{wrapper}: {r:?}");
    }
}
