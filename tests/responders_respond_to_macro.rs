//! Class-level `respond_to :json` (the responders gem) — rubys/roundhouse
//! ranked-gap #8.
//!
//! The responders gem's class-level `respond_to` only configures which
//! mimes `respond_with` answers — a pure declaration with no runtime
//! effect of its own. `report_unrecognized_controller_macros` used to
//! flag every blockless, unrecognized class-body call the same way,
//! so a controller writing `respond_to :json` with no `respond_with`
//! anywhere in its ancestor chain recorded "controller class-body
//! macro not recognized: `respond_to`" under survey, for a macro that
//! does nothing either way.
//!
//! Distinct from the ACTION-body `respond_to do |format| … end` block
//! (`unwrap_respond_to`/PR #843's negotiation), which is unrelated and
//! carries a block — this call never reaches the blockless branch
//! `respond_to_macro_is_consumed` guards.
use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::{ingest_app_from_tree, survey};

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n";
const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\nend\n";

fn respond_to_gaps(files: &[(&str, &str)]) -> (roundhouse::App, Vec<String>) {
    survey::activate();
    let app = ingest_app_from_tree(tree(files)).expect("survey-mode ingest should recover, not abort");
    let gaps = survey::drain();
    let messages: Vec<String> = gaps
        .iter()
        .map(|g| match g {
            roundhouse::ingest::IngestError::Unsupported { message, .. } => message.clone(),
            other => other.to_string(),
        })
        .collect();
    (app, messages)
}

/// `respond_to :json` with plain actions and no `respond_with`
/// anywhere in the chain: consumed as a no-op, no gap, and the
/// controller still ingests/lowers/emits its actions with zero errors.
#[test]
fn respond_to_json_with_no_respond_with_records_no_gap_and_actions_still_work() {
    let files: &[(&str, &str)] = &[
        ("db/schema.rb", SCHEMA),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        (
            "app/controllers/gadget_tags_controller.rb",
            "class GadgetTagsController < ApplicationController\n  respond_to :json\n\n  def index\n    render plain: \"index\"\n  end\n\n  def show\n    render plain: params[:id]\n  end\nend\n",
        ),
    ];
    let (mut app, messages) = respond_to_gaps(files);
    assert!(
        !messages.iter().any(|m| m.contains("respond_to")),
        "class-level `respond_to :json` with no `respond_with` must not be a gap, got: {messages:?}"
    );

    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "GadgetTagsController")
        .expect("controller ingested");
    let action_names: Vec<&str> = controller.actions().map(|a| a.name.as_str()).collect();
    assert!(action_names.contains(&"index") && action_names.contains(&"show"), "{action_names:?}");

    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = diagnose(&app)
        .iter()
        .chain(&lower_diags)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(errors.is_empty(), "actions must still analyze/lower cleanly, got: {errors:?}");
}

/// `respond_to :json` alongside a `respond_with` call anywhere in the
/// controller's ancestor chain: `respond_with` is not modeled, so the
/// macro keeps its gap rather than being silently consumed.
#[test]
fn respond_to_json_with_a_respond_with_call_still_gets_the_gap() {
    let files: &[(&str, &str)] = &[
        ("db/schema.rb", SCHEMA),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        (
            "app/controllers/gadget_tags_controller.rb",
            "class GadgetTagsController < ApplicationController\n  respond_to :json\n\n  def create\n    @tag = :made_up\n    respond_with(@tag)\n  end\nend\n",
        ),
    ];
    let (_app, messages) = respond_to_gaps(files);
    assert!(
        messages.iter().any(|m| m.contains("respond_to")),
        "a `respond_with` call in the chain must keep the `respond_to` gap, got: {messages:?}"
    );
}
