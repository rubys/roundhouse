//! A class-body call with an attached block that NO lowering claims
//! used to vanish from the survey entirely:
//! `report_unrecognized_controller_macros` only ever examined BLOCKLESS
//! calls (`block: None`), so anything with a `do … end` attached —
//! recognized or not — fell outside its reach. That is the other half
//! of rubys/roundhouse#778's bug shape: a block-form `around_action` was
//! one instance of it, but the gate was generic, not specific to
//! `around_action`.
//!
//! This widens the same function to look at blocked calls too, excluded
//! by name for every shape some OTHER lowering already claims
//! (`before_action`/`after_action`/`prepend_before_action`/`around_action`
//! block forms, `rescue_from`/`helper_method`/`layout` via the existing
//! `CONSUMED_CONTROLLER_MACROS` list, `respond_to`), so only a genuinely
//! unclaimed block — a made-up `custom_dsl do … end` — earns a NEW gap.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};

fn gaps_for(widgets_body: &str) -> Vec<IngestError> {
    let files: Vec<(&str, String)> = vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets_body.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    result.expect("ingest must not hard-fail");
    gaps
}

fn has_gap(gaps: &[IngestError], needle: &str) -> bool {
    gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains(needle)))
}

#[test]
fn an_unclaimed_class_body_block_records_a_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  custom_dsl(:loud) do |value|
    value.to_s
  end

  def index
  end
end
"#,
    );
    assert!(
        has_gap(&gaps, "controller class-body block not recognized: `custom_dsl`"),
        "{gaps:?}"
    );
}

#[test]
fn a_before_action_block_form_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  before_action do
    @seen = true
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized") && message.contains("before_action"))),
        "{gaps:?}"
    );
}

#[test]
fn an_after_action_block_form_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  after_action do
    @seen = true
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized") && message.contains("after_action"))),
        "{gaps:?}"
    );
}

#[test]
fn an_accepted_around_action_block_form_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    block.call
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "an accepted around_action lowers away entirely — no gap of any kind: {gaps:?}"
    );
}

/// A REFUSED block-form around_action already earns its own specific
/// gap from `around_block_filter` (see tests/around_block_filters.rs);
/// this generic pass must not ALSO flag it under the generic bucket,
/// which would be a confusing second, differently-worded gap for the
/// same one statement.
#[test]
fn a_refused_around_action_block_form_records_only_its_own_specific_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block, extra|
    block.call
  end

  def index
  end
end
"#,
    );
    assert!(has_gap(&gaps, "around_action block declares 3 parameters"), "{gaps:?}");
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "a refused around_action must not ALSO earn the generic gap: {gaps:?}"
    );
}

#[test]
fn a_rescue_from_block_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  rescue_from(StandardError) { head :internal_server_error }

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "{gaps:?}"
    );
}

#[test]
fn a_layout_block_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  layout do |controller|
    controller.turbo_frame_request? ? false : "application"
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "{gaps:?}"
    );
}

/// Not a real Rails idiom (`respond_to`'s block form is written inside
/// an ACTION body, never bare in the class body), but Ruby syntax
/// allows any call to carry a block, so this is checked defensively.
#[test]
fn a_class_body_respond_to_block_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  respond_to do |format|
    format.html
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "{gaps:?}"
    );
}

#[test]
fn a_prepend_before_action_block_form_records_no_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  prepend_before_action do
    @seen = true
  end

  def index
  end
end
"#,
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized") && message.contains("prepend_before_action"))),
        "{gaps:?}"
    );
}

/// `before_action`/`after_action`/`prepend_before_action` are NOT
/// skipped by NAME alone (unlike `around_action`, whose every refusal
/// already records its own gap via `around_block_filter`):
/// `lambda_filter_target` can decline one of these silently.
/// `before_action(&callback)` forwards an existing Proc bound to a
/// local — `ir_lambda_body` reads a literal `-> { }`/`{ }`/`lambda { }`/
/// `proc { }`, never a forwarded `&var` (that slot holds a bare `Var`,
/// not a `Lambda`) — so `lambda_filter_target` returns `None` for it,
/// and before this fix the call simply vanished with no gap at all
/// (caught declining it today: this assertion fails without the fix).
#[test]
fn a_before_action_block_lambda_filter_target_declines_records_the_generic_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  callback = proc { @seen = true }
  before_action(&callback)

  def index
  end
end
"#,
    );
    assert!(
        has_gap(&gaps, "controller class-body block not recognized: `before_action`"),
        "{gaps:?}"
    );
}

/// A hand-written block filter whose body holds a `next` that can't be
/// restructured to an if/unless (#779, `next_restructure_refusal`)
/// already earns its OWN located gap at ingest time — and the
/// statement never becomes a body item at all (ingest returns `Err`,
/// which the caller records and drops, pushing nothing to the
/// controller's body). So this generic pass's widened before_action/
/// after_action/prepend_before_action handling must not find — and
/// must not double — a gap for it: exactly one gap, worded for the
/// `next` cause, not the generic one.
#[test]
fn a_before_action_block_with_an_unrestructurable_next_records_exactly_one_gap() {
    let gaps = gaps_for(
        r#"class WidgetsController < ApplicationController
  before_action do
    next 1 if admin?
    @seen = true
  end

  def index
  end

  private

  def admin?
    true
  end
end
"#,
    );
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(
        has_gap(&gaps, "`before_action` block holds a `next` that can't be restructured"),
        "{gaps:?}"
    );
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("class-body block not recognized"))),
        "must not ALSO earn the generic gap: {gaps:?}"
    );
}
