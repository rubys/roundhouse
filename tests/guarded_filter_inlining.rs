//! A `before_action` guarded by `if:`/`unless:` cannot be spliced into
//! the action body the way an unconditional one can — the guard is a
//! runtime predicate, not something the splice site can evaluate.
//!
//! `inline_before_filters` used to inline every own-private filter
//! target regardless of `if:`/`unless:`, so `before_action
//! :reject_paging, if: :paging?` answered 401 for every request: the
//! guard was dropped, not evaluated. The harmful direction is just as
//! real — `if: :public?` on a cache-header filter would mark every
//! response publicly cacheable.
//!
//! A guarded filter is now left for `build_filter_preamble`, which
//! already turns `if:`/`unless:` into a runtime condition
//! (`cond_from_guards`) around the dispatch call.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

fn app(controller: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/widgets_controller.rb", controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n",
        ),
    ]))
    .expect("ingest")
}

fn emitted(app: &roundhouse::App) -> String {
    let files = ruby::emit_lowered_controllers(app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .expect("widgets_controller")
}

fn method_body<'a>(out: &'a str, def_line: &str) -> String {
    out.split(def_line)
        .nth(1)
        .unwrap_or_else(|| panic!("{def_line} not found:\n{out}"))
        .split("\n  end")
        .next()
        .unwrap()
        .to_string()
}

const GUARDED_IF: &str = r#"class WidgetsController < ApplicationController
  before_action :reject_paging, if: :paging?

  def index
    head :ok
  end

  private
    def paging?
      params[:page].present?
    end

    def reject_paging
      head :unauthorized
    end
end
"#;

const GUARDED_UNLESS: &str = r#"class WidgetsController < ApplicationController
  before_action :reject_anonymous, unless: :signed_in?

  def index
    head :ok
  end

  private
    def signed_in?
      params[:token].present?
    end

    def reject_anonymous
      head :unauthorized
    end
end
"#;

const UNGUARDED: &str = r#"class WidgetsController < ApplicationController
  before_action :set_widget

  def index
    head :ok
  end

  private
    def set_widget
      @widget = params[:id]
    end
end
"#;

/// The bug: an `if:`-guarded filter's body must NOT land in the action
/// unconditionally — that answered 401 whether or not `page` was
/// present.
#[test]
fn a_guarded_if_filter_is_not_inlined() {
    let out = emitted(&app(GUARDED_IF));
    let index = method_body(&out, "def index");
    assert!(
        !index.contains("unauthorized"),
        "an `if:`-guarded filter must not be spliced into the action body unconditionally:\n{index}"
    );
}

#[test]
fn a_guarded_unless_filter_is_not_inlined() {
    let out = emitted(&app(GUARDED_UNLESS));
    let index = method_body(&out, "def index");
    assert!(
        !index.contains("unauthorized"),
        "an `unless:`-guarded filter must not be spliced into the action body unconditionally:\n{index}"
    );
}

/// The dispatcher preamble is where a guarded filter now runs — gated
/// by the same condition Rails would check.
#[test]
fn a_guarded_if_filter_runs_from_the_preamble_with_its_condition() {
    let out = emitted(&app(GUARDED_IF));
    let dispatch = method_body(&out, "def process_action");
    assert!(
        dispatch.contains("reject_paging"),
        "the guarded filter must still run, from the preamble:\n{dispatch}"
    );
    assert!(dispatch.contains("paging?"), "gated by its `if:` condition:\n{dispatch}");
}

#[test]
fn a_guarded_unless_filter_runs_from_the_preamble_with_its_condition() {
    let out = emitted(&app(GUARDED_UNLESS));
    let dispatch = method_body(&out, "def process_action");
    assert!(
        dispatch.contains("reject_anonymous"),
        "the guarded filter must still run, from the preamble:\n{dispatch}"
    );
    assert!(
        dispatch.contains("signed_in?"),
        "gated by its negated `unless:` condition:\n{dispatch}"
    );
}

/// A guarded filter's target method is still called BY NAME from the
/// preamble, so it must survive — it is no longer "purely consumed by
/// inlining".
#[test]
fn a_guarded_filter_target_method_survives() {
    let out = emitted(&app(GUARDED_IF));
    assert!(out.contains("def reject_paging"), "reject_paging must survive:\n{out}");
}

/// An UNGUARDED own-private filter still inlines exactly as before —
/// this fix must not regress the ordinary case.
#[test]
fn an_unguarded_filter_still_inlines() {
    let out = emitted(&app(UNGUARDED));
    let index = method_body(&out, "def index");
    assert!(
        index.contains("@widget ="),
        "an unguarded private filter still inlines into the action body:\n{index}"
    );
    assert!(
        !out.contains("def set_widget"),
        "and its target, purely a filter target, is still dropped:\n{out}"
    );
}
