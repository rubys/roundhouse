//! Each filter macro invocation binds its own parameters and locals, as
//! each Ruby method call does. The emitted apps run as Ruby, and every
//! expectation is what native Rails 8.1 answers for the same source.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n include Outer\n include Inner\nend\n";

fn concern(name: &str, methods: &str) -> String {
    format!("module {name}\n extend ActiveSupport::Concern\n class_methods do\n {methods}\n end\nend\n")
}

fn controller(call: &str) -> String {
    format!("class ThingsController < ApplicationController\n {call}\n def show\n render plain: 'protected action'\n end\nend\n")
}

fn app(outer: &str, inner: &str, call: &str, assertion: &str) -> emit_and_run::Run {
    emit_and_run::empty_app()
        .write("app/controllers/concerns/outer.rb", &concern("Outer", outer))
        .write("app/controllers/concerns/inner.rb", &concern("Inner", inner))
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/controllers/things_controller.rb", &controller(call))
        .write("config/routes.rb", "Rails.application.routes.draw do\n get '/things' => 'things#show'\nend\n")
        .write(
            "test/controllers/things_controller_test.rb",
            &format!("require 'test_helper'\nclass ThingsControllerTest < ActionDispatch::IntegrationTest\n test 'behavior matches Rails' do\n get '/things'\n {assertion}\n end\nend\n"),
        )
        .run_test("test/controllers/things_controller_test.rb")
}

/// Ingest under `survey`, returning the app and the recorded gaps.
fn ingest(outer: &str, inner: &str, call: &str) -> (App, Vec<IngestError>) {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/outer.rb", concern("Outer", outer)),
        ("app/controllers/concerns/inner.rb", concern("Inner", inner)),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/things_controller.rb", controller(call)),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.into_bytes()))
    .collect();
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    (result.expect("a refused macro is a survey gap, not an ingest failure"), gaps)
}

/// The macro call stays whole, a gap names it, and none of its filters
/// were registered.
fn assert_refused(app: &App, gaps: &[IngestError], macro_name: &str) {
    let things = app.controllers.iter().find(|c| c.name.0.as_str() == "ThingsController").unwrap();
    assert!(
        things.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == macro_name))),
        "the `{macro_name}` call must stay whole"
    );
    assert!(
        !things.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { filter, .. }
            if filter.kind == FilterKind::Before)),
        "no filter from a refused macro"
    );
    let needle = format!("class-body macro not expanded: `{macro_name}`");
    assert!(gaps.iter().any(|gap| gap.to_string().contains(&needle)), "{gaps:?}");
}

#[test]
fn a_reassigned_parameter_leaves_a_parameter_bound_to_the_same_value_unchanged() {
    app(
        "def outer(date)\n inner sunset: date, original: date\nend",
        "def inner(sunset:, original:)\n sunset = sunset&.to_date&.httpdate\n before_action { headers[\"Sunset\"] = sunset; headers[\"Original\"] = original }\nend",
        "outer '2026-01-01'",
        "assert_equal 'Thu, 01 Jan 2026 00:00:00 GMT', response.headers['Sunset']\n assert_equal '2026-01-01', response.headers['Original']",
    )
    .assert_passes();
}

#[test]
fn a_local_reassignment_reads_its_previous_value() {
    app(
        "def outer\n actions = :show\n actions = actions\n inner only: actions\nend",
        "def inner(**options)\n before_action(**options) { headers['Scope'] = 'show' }\nend",
        "outer",
        "assert_equal 'show', response.headers['Scope']\n assert_equal 'protected action', response.body",
    )
    .assert_passes();
}

#[test]
fn an_array_local_is_passed_to_a_nested_macro() {
    app(
        "def outer\n actions = [:show, :index]\n inner only: actions\nend",
        "def inner(**options)\n before_action(**options) { headers['Scope'] = 'show' }\nend",
        "outer",
        "assert_equal 'show', response.headers['Scope']",
    )
    .assert_passes();
}

#[test]
fn an_inner_invocation_can_initialize_locals_after_an_outer_filter() {
    app(
        "def outer\n before_action { headers['Outer'] = 'outer' }\n inner\nend",
        "def inner\n actions = :show\n before_action(only: actions) { headers['Inner'] = 'inner' }\nend",
        "outer",
        "assert_equal 'outer', response.headers['Outer']\n assert_equal 'inner', response.headers['Inner']",
    )
    .assert_passes();
}

#[test]
fn a_rebound_parameter_is_passed_to_the_nested_invocation() {
    app(
        "def outer(date)\n date = date&.to_date&.httpdate\n inner date\nend",
        "def inner(date)\n before_action { headers['Date'] = date }\nend",
        "outer '2026-01-01'",
        "assert_equal 'Thu, 01 Jan 2026 00:00:00 GMT', response.headers['Date']",
    )
    .assert_passes();
}

#[test]
fn replacing_a_caller_parameter_uses_the_new_scope() {
    app(
        "def outer(actions)\n actions = :show\n inner only: actions\nend",
        "def inner(**options)\n before_action(**options) { headers['Scope'] = 'show' }\nend",
        "outer :destroy",
        "assert_equal 'show', response.headers['Scope']",
    )
    .assert_passes();
}

#[test]
fn replacing_an_inner_parameter_does_not_reuse_its_argument() {
    app(
        "def outer\n inner :destroy\nend",
        "def inner(actions)\n actions = :show\n before_action(only: actions) { headers['Scope'] = 'show' }\nend",
        "outer",
        "assert_equal 'show', response.headers['Scope']",
    )
    .assert_passes();
}

#[test]
fn forwarded_keyword_bundles_bind_named_parameters() {
    app(
        "def outer(**options)\n inner **options\nend",
        "def inner(scope:)\n before_action(only: scope) { headers['Scope'] = 'show' }\nend",
        "outer scope: :show",
        "assert_equal 'show', response.headers['Scope']",
    )
    .assert_passes();
}

/// Rails calls a one-parameter `if:` lambda with the controller, so the
/// lambda's own `enabled` is truthy and the filter runs. Substituting the
/// caller's `enabled = false` into it would disable the filter. Nothing
/// here can pass a lambda's parameter as a value, so the macro is refused.
#[test]
fn a_lambda_parameter_is_not_replaced_by_a_caller_local() {
    let (app, gaps) = ingest(
        "def outer\n enabled = false\n inner ->(enabled) { enabled }\nend",
        "def inner(predicate)\n before_action(if: predicate) { head :unauthorized }\nend",
        "outer",
    );
    assert_refused(&app, &gaps, "outer");
}

/// The first callback flips `enabled` in the closure the second one reads.
/// A copied `false` would never see that write.
#[test]
fn a_callback_that_writes_a_bound_name_refuses_the_macro() {
    for write in ["enabled = true", "enabled ||= true", "enabled[:on] = true", "enabled.on = true"] {
        let (app, gaps) = ingest(
            &format!("def outer\n enabled = false\n before_action {{ {write} }}\n inner -> {{ enabled }}\nend"),
            "def inner(predicate)\n before_action(if: predicate) { head :unauthorized }\nend",
            "outer",
        );
        assert_refused(&app, &gaps, "outer");
    }
}

/// A block parameter with a bound name is the block's own, so writing it
/// changes nothing the macro bound.
#[test]
fn a_callback_may_write_its_own_parameter() {
    let (app, gaps) = ingest(
        "def outer\n enabled = :show\n before_action(only: enabled) { [1].each { |enabled| enabled = nil } }\nend",
        "",
        "outer",
    );
    let things = app.controllers.iter().find(|c| c.name.0.as_str() == "ThingsController").unwrap();
    assert!(
        !things.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "outer"))),
        "{gaps:?}"
    );
}
