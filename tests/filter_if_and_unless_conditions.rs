//! A controller filter with BOTH a Symbol `if:` and a Symbol `unless:`
//! guard, written directly (no `with_options` involved) — the shape
//! `ingest_controller_with_options`'s replay produces once a `with_options
//! unless: :signed_in? do before_action :x, if: :paging? end` merges the
//! two onto one call (`rubys/roundhouse#785`, `controller_with_options.
//! rs`).
//!
//! `dialect::Filter` already carries `if_cond` and `unless_cond` as two
//! independent `Option<Symbol>` fields, and `cond_from_guards` in
//! `lower/controller_to_library/process_action.rs` already ANDs
//! whichever of the four guard expressions (`only`/`except`/`if`/
//! `unless`, each in its Symbol or lambda spelling) are present — so
//! this is confirmed already correct end to end, not something the
//! with_options fix had to add. These tests exist to pin that down on
//! its own, independent of with_options composing.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ControllerBodyItem;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(controller_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def index\n    head :ok\n  end\n\n{controller_body}\nend\n"
    );
    [
        ("db/schema.rb".to_string(), "ActiveRecord::Schema.define do\nend\n".to_string()),
        (
            "config/routes.rb".to_string(),
            "Rails.application.routes.draw do\n  get \"/widgets\" => \"widgets#index\"\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb".to_string(),
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/widgets_controller.rb".to_string(), controller),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect()
}

#[test]
fn a_filter_with_both_a_symbol_if_and_a_symbol_unless_keeps_both() {
    let app = ingest_app_from_tree(tree(
        "  before_action :guard, if: :paging?, unless: :signed_in?\n\n  def paging?\n    params[:page].present?\n  end\n\n  def signed_in?\n    params[:user].present?\n  end\n\n  def guard\n    head :unauthorized\n  end\n",
    ))
    .expect("ingest");
    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "WidgetsController")
        .expect("WidgetsController ingested");
    let filter = controller
        .body
        .iter()
        .find_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        })
        .expect("filter ingested");
    assert_eq!(filter.if_cond.as_ref().map(|s| s.as_str()), Some("paging?"), "{filter:#?}");
    assert_eq!(filter.unless_cond.as_ref().map(|s| s.as_str()), Some("signed_in?"), "{filter:#?}");
}

#[test]
fn the_emitted_guard_ands_both_conditions() {
    let mut app = ingest_app_from_tree(tree(
        "  before_action :guard, if: :paging?, unless: :signed_in?\n\n  def paging?\n    params[:page].present?\n  end\n\n  def signed_in?\n    params[:user].present?\n  end\n\n  def guard\n    head :unauthorized\n  end\n",
    ))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .expect("controller emitted")
        .content;
    assert!(
        src.contains("self.guard if self.paging? && !(self.signed_in?)"),
        "{src}"
    );
}

/// The four combinations under CRuby: the guard only fires when its
/// `if:` is true AND its `unless:` is false.
#[test]
fn only_the_if_true_unless_false_combination_rejects_the_request() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/widgets\" => \"widgets#index\"\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :guard, if: :paging?, unless: :signed_in?

  def index
    render plain: "ok"
  end

  def signed_in?
    params[:user].present?
  end

  def paging?
    params[:page].present?
  end

  def guard
    head :unauthorized
  end
end
"#,
        )
        .write(
            "test/controllers/widgets_controller_test.rb",
            r#"require "test_helper"

class WidgetsControllerTest < ActionDispatch::IntegrationTest
  test "if true, unless false: the guard fires" do
    get "/widgets", params: { page: 2 }
    assert_response :unauthorized
  end

  test "if true, unless true: the guard is skipped" do
    get "/widgets", params: { page: 2, user: 1 }
    assert_response :success
  end

  test "if false, unless false: the guard is skipped" do
    get "/widgets"
    assert_response :success
  end

  test "if false, unless true: the guard is skipped" do
    get "/widgets", params: { user: 1 }
    assert_response :success
  end
end
"#,
        )
        .run_test("test/controllers/widgets_controller_test.rb")
        .assert_passes();
}
