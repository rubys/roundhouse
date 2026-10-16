//! `with_options` in a controller's own class body (ActiveSupport's
//! `Object#with_options`): every receiverless, block-free call inside
//! the block receives the block's options, deep-merged under its own
//! (the call's keys win). `rubys/roundhouse#785`:
//!
//! ```ruby
//! class WidgetsController < ApplicationController
//!   with_options unless: :signed_in? do
//!     before_action :reject_paging, if: :paging?
//!     before_action :reject_remote, if: :remote?
//!   end
//! end
//! ```
//!
//! used to fall through to an `Unknown` body item with a block attached,
//! which `report_unrecognized_controller_macros` never names (it only
//! walks block-free sends), so the survey recorded no gap and the whole
//! group — both `before_action`s — was dropped from the emitted
//! `process_action`. An unauthenticated `GET /widgets?page=2` then
//! answered 200 where Rails answers 401.
//!
//! Composed the same way `config/routes.rb`'s `with_options` is
//! (`route_with_options_runs.rs`, `ingest/routes.rs`): each call is
//! re-read with the merged options spliced into its source and ingested
//! as if it had been spelled that way. See `ingest_controller_with_options`
//! in `ingest/controller.rs`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, Filter};
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(controller_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def index\n    head :ok\n  end\n\n{controller_body}\nend\n"
    );
    [
        (
            "db/schema.rb".to_string(),
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n".to_string(),
        ),
        (
            "config/routes.rb".to_string(),
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb".to_string(),
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/models/widget.rb".to_string(), "class Widget < ApplicationRecord\nend\n".to_string()),
        ("app/controllers/widgets_controller.rb".to_string(), controller),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect()
}

/// The `Filter` body items of `WidgetsController`, in source order, from
/// a strict ingest (panics on any gap).
fn filters(controller_body: &str) -> Vec<Filter> {
    let app = ingest_app_from_tree(tree(controller_body)).expect("strict ingest");
    app.controllers
        .iter()
        .find(|c| c.name.0.as_str() == "WidgetsController")
        .expect("WidgetsController ingested")
        .body
        .iter()
        .filter_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some(filter.clone()),
            _ => None,
        })
        .collect()
}

fn strict_error(controller_body: &str) -> String {
    match ingest_app_from_tree(tree(controller_body)) {
        Ok(_) => panic!("strict ingest accepted an unmodeled with_options shape:\n{controller_body}"),
        Err(err) => err.to_string(),
    }
}

/// The survey gaps a (tolerant) ingest of `controller_body` records.
/// Survey mode still returns `Ok` for an unmodeled shape — the gap is
/// what records the decline, not the ingest's own result.
fn survey_gaps(controller_body: &str) -> Vec<String> {
    roundhouse::ingest::survey::activate();
    let _ = ingest_app_from_tree(tree(controller_body)).expect("survey ingest tolerates gaps");
    roundhouse::ingest::survey::drain().iter().map(|g| format!("{g:?}")).collect()
}

#[test]
fn with_options_merges_into_every_call_own_if_survives() {
    let filters = filters(
        "  with_options unless: :signed_in? do\n    before_action :reject_paging, if: :paging?\n    before_action :reject_remote, if: :remote?\n  end\n",
    );
    assert_eq!(filters.len(), 2, "{filters:#?}");
    for filter in &filters {
        assert_eq!(filter.unless_cond.as_ref().map(|s| s.as_str()), Some("signed_in?"), "{filter:#?}");
    }
    assert_eq!(filters[0].target.as_str(), "reject_paging");
    assert_eq!(filters[0].if_cond.as_ref().map(|s| s.as_str()), Some("paging?"), "{:#?}", filters[0]);
    assert_eq!(filters[1].target.as_str(), "reject_remote");
    assert_eq!(filters[1].if_cond.as_ref().map(|s| s.as_str()), Some("remote?"), "{:#?}", filters[1]);
}

/// A successfully composed `with_options` earns no gap at all — not even
/// the generic "class-body block not recognized" one #784 added for an
/// unclaimed block, since the whole statement is gone from `Unknown`
/// before that scan ever runs (folded into the `Filter` items its block
/// held).
#[test]
fn a_composed_with_options_block_records_no_gap() {
    let gaps = survey_gaps(
        "  with_options unless: :signed_in? do\n    before_action :reject_paging, if: :paging?\n  end\n",
    );
    assert!(gaps.is_empty(), "{gaps:#?}");
}

/// The call's own key wins over the outer one under the same key — same
/// `deep_merge` semantics `config/routes.rb`'s `with_options` composes
/// under.
#[test]
fn the_calls_own_key_wins_over_the_outer_one() {
    let filters = filters(
        "  with_options only: [:a, :b] do\n    before_action :x, only: [:c]\n  end\n",
    );
    assert_eq!(filters.len(), 1, "{filters:#?}");
    assert_eq!(
        filters[0].only.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["c"],
        "{:#?}",
        filters[0]
    );
}

/// The call's own key wins even when both are the SAME guard kind: Rails'
/// `OptionMerger`, forwarding each call, does `@options.deep_merge(arguments.
/// pop)` — `Hash#deep_merge` replaces a non-Hash value under a shared
/// key with the incoming one rather than combining them, so the outer
/// `if: :a` is gone entirely once the call's own `if: :b` deep-merges
/// over it (verified against `activesupport/lib/active_support/
/// option_merger.rb` at v8.1.0). `Filter` only ever holds one `if_cond`,
/// so this is also the one shape where losing the outer condition is
/// correct rather than a drop.
#[test]
fn the_calls_own_if_wins_over_the_outer_if_of_the_same_kind() {
    let filters = filters(
        "  with_options if: :a? do\n    before_action :x, if: :b?\n  end\n\n  def a?\n    true\n  end\n\n  def b?\n    true\n  end\n",
    );
    assert_eq!(filters.len(), 1, "{filters:#?}");
    assert_eq!(filters[0].if_cond.as_ref().map(|s| s.as_str()), Some("b?"), "{:#?}", filters[0]);
}

/// Nested `with_options` compose: the innermost call ends up with both
/// ancestors' options, own keys winning at each level.
#[test]
fn nested_with_options_compose() {
    let filters = filters(
        "  with_options unless: :signed_in? do\n    with_options if: :paging? do\n      before_action :reject_paging\n    end\n  end\n",
    );
    assert_eq!(filters.len(), 1, "{filters:#?}");
    assert_eq!(filters[0].target.as_str(), "reject_paging");
    assert_eq!(filters[0].unless_cond.as_ref().map(|s| s.as_str()), Some("signed_in?"));
    assert_eq!(filters[0].if_cond.as_ref().map(|s| s.as_str()), Some("paging?"));
}

/// Every declined shape records a located, specific gap (never the
/// generic "controller class-body macro not recognized" bucket a
/// block-attached `Unknown` item never even reaches) and fails strict
/// ingest the same way.
#[test]
fn unmodeled_with_options_shapes_are_declined() {
    for (body, detail) in [
        (
            "  with_options(unless: :signed_in?) do |o|\n    before_action :x\n  end\n",
            "a block with a parameter",
        ),
        (
            "  with_options OPTS do\n    before_action :x\n  end\n",
            "options must be a literal hash",
        ),
        (
            "  with_options foo: { a: 1 } do\n    before_action :x, foo: { b: 2 }\n  end\n",
            "deep-merging two hashes under `foo:`",
        ),
        (
            "  with_options unless: :signed_in? do\n    if FOO\n      before_action :x\n    end\n  end\n",
            "only receiverless calls",
        ),
        (
            "  with_options unless: :signed_in? do\n    before_action :x, { if: :y }\n  end\n",
            "a positional option hash is not composed",
        ),
        (
            "  with_options only: [:a] do\n    before_action :x, **opts\n  end\n",
            "options splat (`**opts`) is not composed",
        ),
        (
            "  with_options only: [:a] do\n    before_action(:x) { @y = 1 }\n  end\n",
            "`before_action` with a block is not composed",
        ),
        (
            "  with_options only: [:a] do\n    before_action(-> { @y = 1 })\n  end\n",
            "a call whose only argument is a lambda is not composed",
        ),
        (
            "  with_options unless: [:a, :b] do\n    before_action :x\n  end\n",
            "a merged `unless:` that is not a Symbol or a lambda/proc is not composed",
        ),
    ] {
        let err = strict_error(body);
        assert!(err.contains(detail), "{body}\n=> {err}");
        let gaps = survey_gaps(body);
        // Exactly ONE gap for the declined with_options statement — its
        // own specific reason, never also #784's generic "class-body
        // block not recognized" (or the pre-#784 non-block "class-body
        // macro not recognized") bucket on top of it. `with_options` is
        // in `CONSUMED_CONTROLLER_MACROS` precisely so that scan defers
        // to the gap this composing already recorded.
        let with_options_gaps: Vec<&String> =
            gaps.iter().filter(|g| g.contains("with_options:")).collect();
        assert_eq!(
            with_options_gaps.len(),
            1,
            "exactly one with_options gap, not zero or doubled: {body}\n=> {gaps:#?}"
        );
        assert!(with_options_gaps[0].contains(detail), "{body}\n=> {gaps:#?}");
        assert!(
            !gaps.iter().any(|g| {
                g.contains("controller class-body macro not recognized")
                    || g.contains("controller class-body block not recognized")
            }),
            "declined with_options must not ALSO fall to a generic bucket: {gaps:#?}"
        );
    }
}

/// A non-literal `only:`/`except:` reaching `parse_filter_call` through
/// a with_options merge — composed either because the call itself wrote
/// it, or because the outer `with_options` hash supplied it and the
/// splice carried it into the call's own trailing hash — declines the
/// WHOLE with_options statement. `parse_filter_call`'s own refusal
/// doesn't carry the `with_options:` prefix (it fires for a plain call
/// too), so this checks the specific message directly rather than
/// reusing `unmodeled_with_options_shapes_are_declined`'s `with_options:`
/// filter.
#[test]
fn a_non_literal_scope_composed_through_with_options_is_declined() {
    for body in [
        "  with_options unless: :signed_in? do\n    before_action :x, only: some_method\n  end\n\n  def x\n  end\n",
        "  with_options unless: :signed_in? do\n    before_action :x, except: SOME_CONST\n  end\n\n  def x\n  end\n",
        "  with_options only: some_method do\n    before_action :x\n  end\n\n  def x\n  end\n",
    ] {
        let err = strict_error(body);
        assert!(
            err.contains("is not a literal Symbol/String or an array of them"),
            "{body}\n=> {err}"
        );
        let gaps = survey_gaps(body);
        let scope_gaps: Vec<&String> =
            gaps.iter().filter(|g| g.contains("is not a literal Symbol/String")).collect();
        assert_eq!(scope_gaps.len(), 1, "exactly one scope gap: {body}\n=> {gaps:#?}");
        assert!(
            !gaps.iter().any(|g| {
                g.contains("controller class-body macro not recognized")
                    || g.contains("controller class-body block not recognized")
            }),
            "declined with_options must not ALSO fall to a generic bucket: {gaps:#?}"
        );
    }
}

/// The emitted `process_action` guards the filter with BOTH conditions
/// — the merged `unless:` and the filter's own `if:` — rather than
/// dropping either. (Public filter targets: a controller with exactly
/// one routed action, a private filter target, AND a Symbol `if:`/
/// `unless:` guard also hits `inline_before_filters`' own preexisting
/// drop of that guard when it inlines the filter body into the action —
/// a separate gap from with_options composing, worth its own issue, not
/// folded into this one.)
#[test]
fn the_emitted_filter_carries_both_conditions() {
    let app = ingest_app_from_tree(tree(
        "  with_options unless: :signed_in? do\n    before_action :reject_paging, if: :paging?\n  end\n\n  def signed_in?\n    params[:user].present?\n  end\n\n  def paging?\n    params[:page].present?\n  end\n\n  def reject_paging\n    head :unauthorized\n  end\n",
    ))
    .expect("ingest");
    let mut app = app;
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .expect("controller emitted")
        .content;
    assert!(
        src.contains("self.reject_paging if self.paging? && !(self.signed_in?)"),
        "{src}"
    );
}

/// The issue's own reduction, run under CRuby: an unauthenticated,
/// paginated request is rejected; a signed-in one is allowed; and a
/// plain unauthenticated request (neither guard fires) is allowed too.
/// (Public filter targets, not `private` as the issue spells them — a
/// controller with one routed action, a private target, and a Symbol
/// guard also hits the preexisting `inline_before_filters` gap noted
/// above, which is not this fix's to carry.)
#[test]
fn an_unauthenticated_paginated_request_is_rejected_a_signed_in_one_is_allowed() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/widgets\" => \"widgets#index\"\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  with_options unless: :signed_in? do
    before_action :reject_paging, if: :paging?
    before_action :reject_remote, if: :remote?
  end

  def index
    render plain: "ok"
  end

  def signed_in?
    params[:user].present?
  end

  def paging?
    params[:page].present?
  end

  def remote?
    params[:remote].present?
  end

  def reject_paging
    head :unauthorized
  end

  def reject_remote
    head :unauthorized
  end
end
"#,
        )
        .write(
            "test/controllers/widgets_controller_test.rb",
            r#"require "test_helper"

class WidgetsControllerTest < ActionDispatch::IntegrationTest
  test "an unauthenticated paginated request is rejected" do
    get "/widgets", params: { page: 2 }
    assert_response :unauthorized
  end

  test "an unauthenticated remote request is rejected" do
    get "/widgets", params: { remote: 1 }
    assert_response :unauthorized
  end

  test "a signed-in paginated request is allowed" do
    get "/widgets", params: { page: 2, user: 1 }
    assert_response :success
  end

  test "an unauthenticated plain request is allowed" do
    get "/widgets"
    assert_response :success
  end
end
"#,
        )
        .run_test("test/controllers/widgets_controller_test.rb")
        .assert_passes();
}
