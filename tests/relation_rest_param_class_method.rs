//! A relation-taking class method with a REST param.
//!
//! `scope_chain::mentions_bare_chain_start` admits a user-written class
//! method with a bare implicit-self query root (`where`/`none`/…) as
//! relation-taking, the same as a declared `scope`
//! (`tests/relation_class_method_scope.rs`). The emitter gives it a
//! trailing `__rel = ActiveRecord::Relation.new(self)` so a call
//! through a scope chain threads the caller's relation in.
//!
//! `def self.those_tagged(*tags) = where(tag: tags)` broke that: the
//! insertion didn't look at the param list, so a method with a REST
//! param got `__rel` appended as a trailing OPTIONAL POSITIONAL anyway —
//! `def self.those_tagged(*tags, __rel = ActiveRecord::Relation.new(self))`
//! — and an optional positional can never follow a splat; Ruby and
//! Prism both call it a syntax error.
//!
//! Fixed shape: a method with a rest param gets `__rel` as an optional
//! KEYWORD instead (legal after a splat), and every call site threading
//! the relation in passes it as `__rel:` too — including the latent
//! "pad a nil into the splat" bug a bare chained call used to have,
//! fixed alongside (see `thread_rel`'s unit tests in
//! `src/lower/scope_chain.rs`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "tag"
    t.boolean "active", null: false
  end
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  scope :active, -> { where(active: true) }
  scope :combo, -> { those_tagged("a") }

  def self.those_tagged(*tags)
    where(tag: tags)
  end

  def self.those_tagged_with(*tags, **opts)
    where(tag: tags)
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def index
    @direct = Widget.those_tagged("a", "b")
    @zero = Widget.those_tagged
    @splat = Widget.those_tagged(*params[:tags])
    @chained = Widget.active.those_tagged("a")
    @chained_zero = Widget.active.those_tagged
    @chained_splat = Widget.active.those_tagged(*params[:tags])
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", WIDGET),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn lowered() -> roundhouse::App {
    let mut app = app();
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

fn model() -> String {
    emitted(&ruby::emit_lowered_models(&lowered()), "app/models/widget.rb")
}

fn model_rbs() -> String {
    emitted(&ruby::emit_lowered_models(&lowered()), "app/models/widget.rbs")
}

fn controller() -> String {
    emitted(&ruby::emit_lowered_controllers(&lowered()), "app/controllers/widgets_controller.rb")
}

/// `source` parses as Ruby; the message names `path`.
fn assert_parses(source: &str, path: &str) {
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

/// The def line: `__rel` is a KEYWORD with a default, after the splat —
/// the only legal ordering. (Before the fix this was
/// `def self.those_tagged(*tags, __rel = ActiveRecord::Relation.new(self))`,
/// a syntax error: an optional positional can't follow a splat.)
#[test]
fn a_rest_param_method_gets_rel_as_a_keyword_not_a_positional() {
    let m = model();
    assert!(
        m.contains("def self.those_tagged(*tags, __rel: ActiveRecord::Relation.new(self))"),
        "__rel is an optional keyword after the splat:\n{m}"
    );
    assert!(
        !m.contains("*tags, __rel ="),
        "never a positional-with-default after the splat:\n{m}"
    );
    assert_parses(&m, "app/models/widget.rb");
}

/// The body roots its bare `where` on `__rel`, exactly like a
/// non-rest relation-taking method.
#[test]
fn the_body_still_roots_on_the_threaded_relation() {
    let m = model();
    let body = m.split("def self.those_tagged").nth(1).unwrap_or_else(|| panic!("{m}"));
    assert!(body.contains("__rel.where"), "body:\n{body}");
}

/// A method called from INSIDE another relation-taking method/scope —
/// `combo`'s own `__rel` is passed through to `those_tagged`, by the
/// same keyword (not positionally, and not re-seeded).
#[test]
fn a_call_from_inside_another_relation_taking_body_threads_rel_to_rel() {
    let m = model();
    assert!(
        m.contains(r#"those_tagged("a", __rel: __rel)"#),
        "combo's own __rel threads into those_tagged by keyword:\n{m}"
    );
}

/// Call-site shapes, straight from the controller.
#[test]
fn direct_zero_arg_and_splat_calls_are_unchanged() {
    let c = controller();
    assert!(c.contains(r#"Widget.those_tagged("a", "b")"#), "direct call untouched:\n{c}");
    assert!(c.contains("@zero = Widget.those_tagged\n"), "bare zero-arg call untouched:\n{c}");
    assert!(
        c.contains("Widget.those_tagged(*self.params[\"tags\"])")
            || c.contains("Widget.those_tagged(*self.params[:tags])")
            || c.contains("Widget.those_tagged(*params[\"tags\"])"),
        "direct splat call untouched (source shape, modulo the ordinary params rewrite):\n{c}"
    );
}

#[test]
fn a_chained_call_threads_the_relation_as_a_keyword() {
    let c = controller();
    assert!(
        c.contains(r#"Widget.those_tagged("a", __rel: Widget.active)"#),
        "chained call re-roots at the constant and threads __rel: as a keyword:\n{c}"
    );
}

/// The fix this commit names explicitly: a chained ZERO-arg call used
/// to pad a spurious positional `nil` into the rest slot before
/// appending the relation. Now: no padding, just the keyword.
#[test]
fn a_chained_zero_arg_call_has_no_nil_padding() {
    let c = controller();
    assert!(
        c.contains("Widget.those_tagged(__rel: Widget.active)"),
        "no nil padded into the rest slot:\n{c}"
    );
    assert!(
        !c.contains("Widget.those_tagged(nil"),
        "the latent nil-pad bug must not resurface:\n{c}"
    );
}

#[test]
fn a_chained_splat_call_threads_the_relation_after_the_splat() {
    let c = controller();
    assert!(
        c.contains("Widget.active)")
            && (c.contains("those_tagged(*self.params[\"tags\"], __rel: Widget.active)")
                || c.contains("those_tagged(*self.params[:tags], __rel: Widget.active)")
                || c.contains("those_tagged(*params[\"tags\"], __rel: Widget.active)")),
        "the splat stays first, __rel: keyword trails it:\n{c}"
    );
}

/// The `.rbs` line. The general signature-stamping pass declines to
/// stamp a rest-param, bare-chain-start class method (same as every
/// OTHER method `push_scope_methods`/this registry synthesizes — none
/// of them carry a `Ty::Fn` signature in this fixture), so the emitted
/// `.rbs` falls through to the untyped-param fallback renderer, which
/// renders EVERY param "untyped" regardless of its actual `Ty` —
/// `__rel` included. That fallback is unconditional and pre-existing
/// (it already did the same for every `__rel` this file's sibling
/// tests see, rest or not), so this is not a gap this fix opened.
/// What the fix actually changes, at the TYPE level, is asserted
/// directly against `insert_rel_param` in `src/emit/ruby/library.rs`'s
/// own unit tests.
#[test]
fn the_rbs_signature_is_unaffected_by_whether_rel_is_a_keyword_or_a_positional() {
    let rbs = model_rbs();
    assert!(
        rbs.contains("def self.those_tagged: (*untyped tags, ?__rel: untyped) -> untyped"),
        "rbs:\n{rbs}"
    );
}

/// No argument-forwarding / splat-vs-kwargs confusion at the Ruby
/// level: the whole model and controller still parse.
#[test]
fn the_whole_model_and_controller_parse() {
    assert_parses(&model(), "app/models/widget.rb");
    assert_parses(&controller(), "app/controllers/widgets_controller.rb");
}

/// `*tags, **opts`: no keyword may follow a `**opts`. Ingest doesn't
/// admit this shape as relation-taking today (it roots on a fresh
/// Relation and gets no `__rel`); `insert_rel_param`'s own unit test
/// pins the placement for the day it does. Here: whatever it emits,
/// `__rel` never lands after the `**opts`, and the model parses.
#[test]
fn a_rest_and_kwrest_method_never_gets_rel_after_the_kwrest() {
    let m = model();
    let def = m
        .lines()
        .find(|l| l.contains("def self.those_tagged_with("))
        .unwrap_or_else(|| panic!("no those_tagged_with def:\n{m}"));
    assert!(!def.contains("**opts, __rel"), "__rel after **opts: {def}");
    assert_parses(&m, "app/models/widget.rb");
}
