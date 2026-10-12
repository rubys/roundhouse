//! A filter's `only:`/`except:` value, when it is NOT a literal Symbol/
//! String or an array of them — `before_action :reject, only:
//! some_method` (Rails calls `some_method` at class-definition time and
//! scopes to whatever action names it returns; `skip_before_action
//! :verify_authenticity_token, only: some_method` — same shape, but
//! disabling CSRF verification on the wrong actions is the sharper
//! failure).
//!
//! `symbol_list_value` reads a non-literal node as an empty list — it
//! has no array or symbol/string shape to extract — and
//! `before_filter_applies`/`cond_from_guards` read an empty `only`/
//! `except` as "every action", not "unknown". So the filter silently
//! applied everywhere instead of refusing a scope the ingester cannot
//! read. `is_recognized_scope_value` (`ingest/controller.rs`) names the
//! shapes it CAN read — a literal Symbol, a literal String, or an array
//! of only those — and `parse_filter_call`/`parse_forgery_macro` refuse
//! the call rather than guess when `only:`/`except:` is anything else.
//!
//! Independent of `with_options` composing (`controller_with_options.rs`
//! pins the with_options-merged version of this same gap down
//! separately): this is the same `parse_filter_call` a plain,
//! unwrapped `before_action`/`skip_before_action`/`protect_from_forgery`
//! call reaches.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, Filter};
use roundhouse::ingest::ingest_app_from_tree;

fn tree(controller_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def index\n    head :ok\n  end\n\n  def show\n    head :ok\n  end\n\n{controller_body}\nend\n"
    );
    [
        ("db/schema.rb".to_string(), "ActiveRecord::Schema.define do\nend\n".to_string()),
        (
            "config/routes.rb".to_string(),
            "Rails.application.routes.draw do\n  get \"/widgets\" => \"widgets#index\"\n  get \"/widgets/:id\" => \"widgets#show\"\nend\n".to_string(),
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
        Ok(_) => panic!("strict ingest accepted a non-literal only:/except::\n{controller_body}"),
        Err(err) => err.to_string(),
    }
}

fn survey_gaps(controller_body: &str) -> Vec<String> {
    roundhouse::ingest::survey::activate();
    let _ = ingest_app_from_tree(tree(controller_body)).expect("survey ingest tolerates gaps");
    roundhouse::ingest::survey::drain().iter().map(|g| format!("{g:?}")).collect()
}

#[test]
fn a_non_literal_only_on_a_plain_before_action_is_refused() {
    for body in [
        "  before_action :reject, only: some_method\n\n  def reject\n    head :unauthorized\n  end\n",
        "  before_action :reject, only: SOME_CONST\n\n  def reject\n    head :unauthorized\n  end\n",
    ] {
        let err = strict_error(body);
        assert!(
            err.contains("`before_action` `only:` is not a literal Symbol/String or an array of them"),
            "{body}\n=> {err}"
        );
        let gaps = survey_gaps(body);
        assert!(
            gaps.iter().any(|g| g.contains("`before_action` `only:` is not a literal Symbol/String")),
            "{body}\n=> {gaps:#?}"
        );
    }
}

#[test]
fn a_non_literal_except_on_a_plain_before_action_is_refused_the_same_way() {
    let body =
        "  before_action :reject, except: some_method\n\n  def reject\n    head :unauthorized\n  end\n";
    let err = strict_error(body);
    assert!(
        err.contains("`before_action` `except:` is not a literal Symbol/String or an array of them"),
        "{body}\n=> {err}"
    );
}

/// The sharper failure: a `skip_before_action` whose scope can't be read
/// would otherwise skip the filter on every action instead of just the
/// ones named, silently disabling whatever it guards more broadly than
/// the source said.
#[test]
fn skip_before_action_with_a_non_literal_only_is_also_refused() {
    let body = "  skip_before_action :some_inherited_filter, only: some_method\n";
    let err = strict_error(body);
    assert!(
        err.contains("`skip_before_action` `only:` is not a literal Symbol/String or an array of them"),
        "{body}\n=> {err}"
    );
}

/// `protect_from_forgery`/`skip_forgery_protection` read `only:`/
/// `except:` through the same `symbol_list_value` call, inside
/// `parse_forgery_macro` rather than `parse_filter_call`'s own loop —
/// same bug, same fix.
#[test]
fn protect_from_forgery_with_a_non_literal_only_is_refused() {
    let body = "  protect_from_forgery with: :exception, only: some_method\n";
    let err = strict_error(body);
    assert!(
        err.contains("`protect_from_forgery` `only:` is not a literal Symbol/String or an array of them"),
        "{body}\n=> {err}"
    );
}

/// Literal shapes are unaffected: a bare Symbol, a String (Rails reads
/// either the same way), and an array of Symbols all still scope and
/// apply exactly as before.
#[test]
fn literal_scope_shapes_still_apply_correctly() {
    let symbol_only =
        filters("  before_action :reject, only: :show\n\n  def reject\n    head :unauthorized\n  end\n");
    assert_eq!(
        symbol_only[0].only.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["show"],
        "{:#?}",
        symbol_only[0]
    );

    let string_only = filters(
        "  before_action :reject, only: \"show\"\n\n  def reject\n    head :unauthorized\n  end\n",
    );
    assert_eq!(
        string_only[0].only.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["show"],
        "{:#?}",
        string_only[0]
    );

    let array_only = filters(
        "  before_action :reject, only: [:index, :show]\n\n  def reject\n    head :unauthorized\n  end\n",
    );
    assert_eq!(
        array_only[0].only.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["index", "show"],
        "{:#?}",
        array_only[0]
    );

    let except_array = filters(
        "  before_action :reject, except: [:index]\n\n  def reject\n    head :unauthorized\n  end\n",
    );
    assert_eq!(
        except_array[0].except.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["index"],
        "{:#?}",
        except_array[0]
    );
}

/// Same gap, reached through a concern's `included do` block instead of
/// a plain controller body: `ingest_concern_filters` (`library_class.rs`)
/// runs `parse_filter_call` over every statement inside `included do
/// ... end` and used to hand its `Err` straight to `survey::record`,
/// which is a no-op outside survey mode — so in strict mode the
/// unreadable scope silently vanished (as if the `before_action` were
/// never there) instead of failing ingest the way the same call written
/// directly in the controller does.
fn concern_tree(included_do_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let concern = format!(
        "module Guarded\n  extend ActiveSupport::Concern\n\n  included do\n{included_do_body}\n  end\nend\n"
    );
    let controller = "class WidgetsController < ApplicationController\n  include Guarded\n\n  def index\n    head :ok\n  end\n\n  def show\n    head :ok\n  end\n\n  def reject\n    head :unauthorized\n  end\nend\n".to_string();
    [
        ("db/schema.rb".to_string(), "ActiveRecord::Schema.define do\nend\n".to_string()),
        (
            "config/routes.rb".to_string(),
            "Rails.application.routes.draw do\n  get \"/widgets\" => \"widgets#index\"\n  get \"/widgets/:id\" => \"widgets#show\"\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb".to_string(),
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/concerns/guarded.rb".to_string(), concern),
        ("app/controllers/widgets_controller.rb".to_string(), controller),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect()
}

fn concern_strict_error(included_do_body: &str) -> String {
    match ingest_app_from_tree(concern_tree(included_do_body)) {
        Ok(_) => {
            panic!("strict ingest accepted a non-literal only:/except: inside a concern's included do:\n{included_do_body}")
        }
        Err(err) => err.to_string(),
    }
}

fn concern_survey_gaps(included_do_body: &str) -> Vec<String> {
    roundhouse::ingest::survey::activate();
    let _ =
        ingest_app_from_tree(concern_tree(included_do_body)).expect("survey ingest tolerates gaps");
    roundhouse::ingest::survey::drain().iter().map(|g| format!("{g:?}")).collect()
}

#[test]
fn a_non_literal_only_inside_a_concerns_included_do_is_refused() {
    let body = "    before_action :reject, only: some_method\n";
    let err = concern_strict_error(body);
    assert!(
        err.contains("`before_action` `only:` is not a literal Symbol/String or an array of them"),
        "{body}\n=> {err}"
    );

    let gaps = concern_survey_gaps(body);
    assert_eq!(
        gaps.len(),
        1,
        "expected exactly one recorded gap for the concern's unreadable scope, got:\n{gaps:#?}"
    );
    assert!(
        gaps[0].contains("`before_action` `only:` is not a literal Symbol/String"),
        "{body}\n=> {gaps:#?}"
    );
}

#[test]
fn a_non_literal_except_inside_a_concerns_included_do_is_refused_the_same_way() {
    let body = "    before_action :reject, except: some_method\n";
    let err = concern_strict_error(body);
    assert!(
        err.contains("`before_action` `except:` is not a literal Symbol/String or an array of them"),
        "{body}\n=> {err}"
    );

    let gaps = concern_survey_gaps(body);
    assert_eq!(
        gaps.len(),
        1,
        "expected exactly one recorded gap for the concern's unreadable scope, got:\n{gaps:#?}"
    );
    assert!(
        gaps[0].contains("`before_action` `except:` is not a literal Symbol/String"),
        "{body}\n=> {gaps:#?}"
    );
}
