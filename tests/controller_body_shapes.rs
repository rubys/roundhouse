//! Controller class-body shapes beyond Symbol-target filter DSL and
//! `def`/`include`: a lambda/block-target `before_action`, and the
//! plain-Ruby class-body macros (`attr_reader`, `alias_method`,
//! `prepend`, `private_constant`, `helper`, `using`) that a
//! controller's own ingest used to fall through to `Unknown` and then
//! flag as "controller class-body macro not recognized".
//!
//! F28 (lambda filters): `before_action -> { … }, only: […]` and
//! `before_action { … }` both register on the guarded action, running
//! the body in controller-instance context — see
//! `ingest::controller::lambda_filter_target`.
//!
//! F21/F29 (plain Ruby): `attr_reader`/`attr_accessor` synthesize real
//! accessor methods, `alias_method` clones the aliased method,
//! `prepend` is read like `include`, `private_constant`/`helper` are
//! silent no-ops, and `using` earns a specific ledger line instead of
//! the generic "not recognized" one.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

const PREPENDED_HELPER: &str = r#"module PrependedHelper
  def prepended_thing
    1
  end
end
"#;

const WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  prepend PrependedHelper
  using PrependedHelper

  before_action -> { ensure_guard }, only: [:show]
  before_action do
    @loaded = true
  end

  attr_reader :x

  def a
    1
  end
  alias_method :b, :a

  K = 1
  private_constant :K

  helper FooHelper

  def show
  end

  def index
  end

  private

  def ensure_guard
    @guarded = true
  end
end
"#;

const FOO_HELPER: &str = "module FooHelper\n  def foo\n    1\n  end\nend\n";

fn tree() -> HashMap<PathBuf, Vec<u8>> {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/concerns/prepended_helper.rb", PREPENDED_HELPER),
        ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
        ("app/helpers/foo_helper.rb", FOO_HELPER),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show]\nend\n",
        ),
    ];
    files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

fn emitted_widgets() -> String {
    let mut app = ingest_app_from_tree(tree()).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("widgets_controller.rb not emitted"))
}

/// `before_action -> { ensure_guard }, only: [:show]` — the argument
/// form, no attached block at all — registers on `show` exactly like
/// a Symbol-target filter would.
#[test]
fn lambda_argument_form_before_action_registers_on_its_guarded_action() {
    let src = emitted_widgets();
    assert!(
        src.contains("ensure_guard"),
        "the lambda filter's body must reach the emitted dispatcher:\n{src}"
    );
    assert!(
        src.contains("[:show].include?(action_name)"),
        "its only: scoping must survive:\n{src}"
    );
}

/// `before_action do … end` — the block-attached form — still
/// registers unconditionally (no `only:`/`except:`).
#[test]
fn block_form_before_action_still_registers() {
    let src = emitted_widgets();
    assert!(src.contains("@loaded = true"), "{src}");
}

/// `attr_reader :x` synthesizes a real `x` method reading `@x`.
#[test]
fn attr_reader_synthesizes_a_real_accessor() {
    let src = emitted_widgets();
    assert!(src.contains("def x"), "{src}");
}

/// `alias_method :b, :a` synthesizes `b` with `a`'s body.
#[test]
fn alias_method_synthesizes_the_aliased_method() {
    let src = emitted_widgets();
    assert!(src.contains("def b"), "{src}");
}

/// `prepend PrependedHelper` — read the same way `include` is: its
/// instance methods are folded onto the controller.
#[test]
fn prepend_makes_the_modules_own_methods_reachable() {
    let src = emitted_widgets();
    assert!(src.contains("def prepended_thing"), "{src}");
}

/// The survey report — activated across the WHOLE pipeline (ingest
/// through lowering) — carries the specific line `using` earns and,
/// crucially, NO generic "controller class-body macro not recognized"
/// line for any of this file's shapes.
#[test]
fn the_survey_report_has_only_the_specific_lines_no_generic_unrecognized() {
    roundhouse::ingest::survey::activate();
    let mut app = ingest_app_from_tree(tree()).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let _ = ruby::emit_lowered_controllers(&app);
    let gaps = roundhouse::ingest::survey::drain();
    let messages: Vec<String> = gaps.iter().map(|g| format!("{g:?}")).collect();

    assert!(
        messages
            .iter()
            .any(|m| m.contains("refinement activated") && m.contains("using PrependedHelper")),
        "using must earn its own line naming the refinement: {messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("controller class-body macro not recognized")),
        "none of these shapes should fall to the generic bucket: {messages:?}"
    );
}
