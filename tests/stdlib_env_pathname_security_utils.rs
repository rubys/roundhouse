//! `ENV.fetch`, `Pathname#relative_path_from`, `Date.parse` and
//! `ActiveSupport::SecurityUtils.secure_compare` — four call shapes an
//! ordinary Rails app writes and the registry had no answer for.
//!
//! Each was a `send_dispatch_failed` against a constant the analyzer
//! knew nothing about, which is a diagnostic about roundhouse rather
//! than about the app: `ENV` is where a Rails app keeps its
//! configuration, `secure_compare` is what a hand-rolled token check
//! calls, and `Rails.root`-relative path arithmetic goes through
//! `Pathname`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn diagnostics_for(action: &str) -> Vec<String> {
    let controller = format!(
        "class ReportsController < ApplicationController\n  def show\n    {action}\n  end\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_string(),
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/reports_controller.rb", controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect()
}

fn dispatch_errors(diagnostics: &[String]) -> Vec<&String> {
    diagnostics
        .iter()
        .filter(|d| d.contains("send_dispatch_failed"))
        .collect()
}

#[test]
fn the_four_shapes_dispatch() {
    for action in [
        "@a = ENV.fetch(\"TOKEN\", \"\")",
        "@a = ENV[\"TOKEN\"]",
        "@a = ENV.to_h",
        "@a = ActiveSupport::SecurityUtils.secure_compare(params[:token].to_s, \"x\")",
        "@a = Pathname.new(\"/tmp/x\").relative_path_from(Pathname.pwd).to_s",
        "@a = Date.parse(params[:date].to_s)",
    ] {
        let diags = diagnostics_for(action);
        assert!(
            dispatch_errors(&diags).is_empty(),
            "{action} should resolve; diagnostics = {diags:?}"
        );
    }
}

#[test]
fn the_answers_carry_their_type_onward() {
    // `ENV` values are Strings, and a path chain stays a Pathname until
    // `to_s` — both have to hold for the call after them to resolve.
    let diags = diagnostics_for("@a = ENV.fetch(\"TOKEN\", \"\").upcase");
    assert!(dispatch_errors(&diags).is_empty(), "ENV.fetch answers a String; {diags:?}");

    let diags =
        diagnostics_for("@a = Pathname.new(\"/tmp/x\").relative_path_from(Pathname.pwd).to_s.upcase");
    assert!(
        dispatch_errors(&diags).is_empty(),
        "the path chain stays a Pathname and ends in a String; {diags:?}"
    );

    // `ENV[...]` is modeled as `String | nil` where `fetch` is a plain
    // String, which is the difference between the two in Ruby. What the
    // analyzer does with an unguarded String call on that union — today,
    // nothing — is its own question and not this registry entry's.
}
