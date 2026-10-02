//! A controller helper's keyword parameters are keywords in its `.rbs`
//! sidecar too, typed from the call sites by name.
//!
//! The `def` kept `describe(name:, count:, unit: "pcs")`, but the
//! sidecar declared three positionals and gave the first one the type
//! of the call's whole kwargs Hash:
//! `(Hash[Symbol, (Integer | String)] name, untyped count, untyped unit)`.
//! spinel compiles against the sidecar, so it refused the call ("a
//! method argument given a String, which no conversion keeps in its
//! sp_SymPolyHash * slot").

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  root \"widgets#index\"\nend\n";

const WIDGETS_CONTROLLER: &str = r##"class WidgetsController < ApplicationController
  def index
    opts = { label: 1, tone: 2 }
    render plain: describe(name: "gear", count: 2) + tagged(label: "x", tone: "red", size: 3) + note("n", **{ a: 1 }) + h(1, x: 2) + named("text", a: 2) + labelled(label: 1, tone: 2) + forwarded(**opts) + spread(1, x: 2)
  end

  private

  def describe(name:, count:, unit: "pcs")
    "#{name} x#{count} #{unit}"
  end

  def tagged(label:, **rest)
    "#{label} #{rest.size}"
  end

  def note(text, **opts)
    "#{text}#{opts.size}"
  end

  def h(a, **opts)
    "#{a}#{opts[:x]}"
  end

  def named(a, **opts)
    "#{a}#{opts[:a]}"
  end

  def labelled(prefix = nil, label:, **rest)
    "#{prefix}#{label}#{rest.size}"
  end

  def forwarded(prefix = nil, label:, **rest)
    "#{prefix}#{label}#{rest.size}"
  end

  def spread(a, b = nil, **rest)
    "#{a}#{b}#{rest.size}"
  end
end
"##;

/// The spinel tree for a small app: the base classes, a `widgets`
/// table, a root route and the controller above.
fn spinel() -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
    ] {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

fn sig<'a>(rbs: &'a str, method: &str) -> &'a str {
    let head = format!("def {method}: ");
    rbs.lines()
        .map(str::trim)
        .find(|l| l.starts_with(&head))
        .unwrap_or_else(|| panic!("no sig for {method}:\n{rbs}"))
}

#[test]
fn the_def_keeps_its_keywords() {
    let files = spinel();
    let rb = file(&files, "app/controllers/widgets_controller.rb");
    assert!(rb.contains("def describe(name:, count:, unit: \"pcs\")"), "{rb}");
    assert!(rb.contains("def tagged(label:, **rest)"), "{rb}");
}

#[test]
fn the_sidecar_declares_keywords_typed_by_name() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    if let Err(e) = ruby_rbs::node::parse(rbs) {
        panic!("not valid RBS: {e}\n{rbs}");
    }
    assert_eq!(sig(rbs, "describe"), "def describe: (name: String, count: Integer, ?unit: untyped) -> untyped");
    assert!(!rbs.contains("Hash[Symbol, (Integer | String)] name"), "{rbs}");
}

#[test]
fn a_keyword_rest_stays_a_keyword_rest() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    let tagged = sig(rbs, "tagged");
    assert!(tagged.starts_with("def tagged: (label: String, "), "{tagged}");
    assert!(tagged.contains("**untyped rest"), "{tagged}");
}

#[test]
fn a_keyword_rest_alone_is_not_a_positional() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    let note = sig(rbs, "note");
    assert!(note.contains("**untyped opts"), "{note}");
}

/// `h(1, x: 2)` against `def h(a, **opts)`: no parameter is named `x`,
/// and the slot the kwargs hash lands on is the `**opts` Ruby binds it
/// to (`a = 1`, `opts = {x: 2}`). The call stays as written, `a` keeps
/// the positional's type and `opts` stays a keyword rest.
#[test]
fn a_key_only_a_keyword_rest_takes_binds_like_ruby() {
    let files = spinel();
    let rb = file(&files, "app/controllers/widgets_controller.rb");
    assert!(ruby_prism::parse(rb.as_bytes()).errors().next().is_none(), "{rb}");
    assert!(rb.contains("def h(a, **opts)"), "{rb}");
    assert!(rb.contains("h(1, x: 2)"), "{rb}");
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert_eq!(sig(rbs, "h"), "def h: (Integer a, **untyped opts) -> untyped");
}

/// `named("text", a: 2)` against `def named(a, **opts)`: a key that
/// shares a positional's name still binds to the keyword rest (`a =
/// "text"`, `opts = {a: 2}`), so `a` keeps the positional's type.
#[test]
fn a_key_named_like_a_positional_binds_to_the_keyword_rest() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert_eq!(sig(rbs, "named"), "def named: (String a, **untyped opts) -> untyped");
}

/// `labelled(label: 1, tone: 2)` against `def labelled(prefix = nil,
/// label:, **rest)`: with no positional passed, Ruby binds `prefix =
/// nil`, `label = 1`, `rest = {tone: 2}`. `label` is typed from the
/// call and `prefix` is not given the kwargs Hash.
#[test]
fn a_keyword_after_an_optional_is_typed_when_only_keywords_are_passed() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert_eq!(sig(rbs, "labelled"), "def labelled: (?untyped prefix, label: Integer, **untyped rest) -> untyped");
}

/// `forwarded(**opts)` against `def forwarded(prefix = nil, label:,
/// **rest)`: a forwarded hash is keyword arguments, not a positional,
/// so Ruby binds `prefix = nil`, `label = 1`, `rest = {tone: 2}`. The
/// hash's keys are not known at the call, so `label` stays untyped, and
/// `prefix` is not given the Hash.
#[test]
fn a_forwarded_keyword_hash_does_not_type_an_optional_positional() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert_eq!(
        sig(rbs, "forwarded"),
        "def forwarded: (?untyped prefix, label: untyped, **untyped rest) -> untyped"
    );
}

/// `spread(1, x: 2)` against `def spread(a, b = nil, **rest)`: Ruby
/// binds `a = 1`, `b = nil`, `rest = {x: 2}`. The kwargs hash belongs
/// to `**rest`, not to the optional positional between them.
#[test]
fn a_kwargs_hash_skips_an_optional_positional_for_the_keyword_rest() {
    let files = spinel();
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert_eq!(
        sig(rbs, "spread"),
        "def spread: (Integer a, ?untyped b, **untyped rest) -> untyped"
    );
}
