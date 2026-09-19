//! `Turbo::StreamsChannel.broadcast_update_to(stream, target:, html:)`
//! — turbo-rails' class-side broadcast API.
//!
//! The record-side surface is mixed into `ActiveRecord::Base`, so every
//! model answers it and `model_to_library::broadcasts` rewrites it to
//! the target-neutral `Broadcasts.<action>`. The class-side form is
//! what an app writes when the payload is not a record's partial — a
//! rendered component, a counter, a status panel — and it was a
//! dispatch error against a constant the registry did not know.
//!
//! The dispatch half — the call reporting `send_dispatch_failed`
//! before this — is measured on a real app (6 errors → 1) rather than
//! pinned here: I could not reproduce the diagnostic in a minimal
//! fixture, and a test that passes with and without the change is
//! worse than none.
//!
//! `update` is new on both sides: Turbo has it (it replaces a target's
//! CONTENTS where `replace` replaces the element), the runtimes did
//! not.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const BROADCASTER: &str = r#"class StatusBroadcaster
  def self.refresh(html)
    Turbo::StreamsChannel.broadcast_update_to(
      "internal",
      target: "status_panel",
      html: html
    )
  end

  def self.drop
    Turbo::StreamsChannel.broadcast_remove_to("internal", target: "status_panel")
  end
end
"#;

fn app_with(source: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/report.rb", "class Report < ApplicationRecord\nend\n"),
        ("app/services/status_broadcaster.rb", source),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted_broadcaster(app: &roundhouse::App) -> String {
    ruby::emit_library(app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("status_broadcaster.rb"))
        .map(|f| f.content)
        .expect("the broadcaster is emitted")
}

#[test]
fn a_class_side_broadcast_lowers_to_the_target_neutral_sink() {
    let app = app_with(BROADCASTER);
    let emitted = emitted_broadcaster(&app);
    assert!(
        emitted.contains("Broadcasts.update(stream: \"internal\", target: \"status_panel\""),
        "the class-side call has to reach `Broadcasts`, which every target's runtime has; got:\n{emitted}"
    );
    assert!(
        emitted.contains("Broadcasts.remove(stream: \"internal\", target: \"status_panel\")"),
        "the html-less action lowers too; got:\n{emitted}"
    );
    assert!(
        !emitted.contains("Turbo::StreamsChannel"),
        "no target but ruby and spinel defines that class; got:\n{emitted}"
    );
}

/// A guard rather than a proof: this passes with or without the
/// lowering, and exists so a later widening of the rewrite cannot
/// quietly start swallowing options it does not carry.
#[test]
fn a_render_option_the_sink_cannot_carry_is_left_alone() {
    // `partial:` renders through Rails rather than carrying markup.
    // Rewriting it would emit a broadcast that silently drops what it
    // was told to render, so the call stays as written and the
    // diagnostics keep naming it.
    let source = r#"class StatusBroadcaster
  def self.refresh(record)
    Turbo::StreamsChannel.broadcast_update_to(
      "internal",
      target: "status_panel",
      partial: "reports/report",
      locals: { report: record }
    )
  end
end
"#;
    let app = app_with(source);
    let emitted = emitted_broadcaster(&app);
    assert!(
        emitted.contains("Turbo::StreamsChannel.broadcast_update_to"),
        "an unlowerable option leaves the call verbatim; got:\n{emitted}"
    );
    assert!(!emitted.contains("Broadcasts.update"), "got:\n{emitted}");
}

/// The stream and the target are taken only as literals. A record
/// streamable is named by `stream_name_from` and an array target is
/// `dom_id(*array)`; the record-side pass spells both because it
/// knows the model, and this pass has none. Passed through raw, the
/// record is a stream key nobody is subscribed to — and on crystal a
/// `stream : String` that does not compile.
#[test]
fn a_record_stream_or_array_target_is_left_alone() {
    let source = r#"class StatusBroadcaster
  def self.refresh(report, html)
    Turbo::StreamsChannel.broadcast_update_to(report, target: "status_panel", html: html)
  end

  def self.refresh_unread(report, html)
    Turbo::StreamsChannel.broadcast_update_to("internal", target: [report, :unread], html: html)
  end

  def self.refresh_symbol(html)
    Turbo::StreamsChannel.broadcast_update_to(:internal, target: "status_panel", html: html)
  end
end
"#;
    let app = app_with(source);
    let emitted = emitted_broadcaster(&app);
    assert!(
        emitted.contains("broadcast_update_to(report, target: \"status_panel\""),
        "a record stream leaves the call verbatim; got:\n{emitted}"
    );
    assert!(
        emitted.contains("target: [report, :unread]"),
        "an array target leaves the call verbatim; got:\n{emitted}"
    );
    assert!(
        emitted.contains("Broadcasts.update(stream: \"internal\", target: \"status_panel\""),
        "a symbol stream is its own name, as `stream_name_from(:internal)` says; got:\n{emitted}"
    );
    assert_eq!(
        emitted.matches("Broadcasts.update").count(),
        1,
        "only the literal form lowers; got:\n{emitted}"
    );
}
