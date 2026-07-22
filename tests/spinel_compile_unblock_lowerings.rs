//! The four lowerings that carried the lobsters spinel-AOT compile from
//! its last analyze wall to a linked binary (see
//! project_lobsters_spinel_aot_probe): literal boolean short-circuit
//! fold, `values_at(*keys)` desugar, scalar return-type backfill on
//! user model methods, and parens around assignment receivers. Each is
//! exercised through the public emit pipeline on a minimal app carrying
//! the exact lobsters shape that failed.

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

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "username", null: false
  end
end
"#;

fn app_with(model_body: &str, controller_body: &str) -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            &format!("class User < ApplicationRecord\n{model_body}\nend\n"),
        ),
        (
            "app/controllers/home_controller.rb",
            &format!(
                "class HomeController < ApplicationController\n{controller_body}\nend\n"
            ),
        ),
    ]))
    .expect("ingest");
    // The fold/desugar passes live on the shared post-analyze hook —
    // the same shape the transpile driver feeds the emitters.
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("no emitted file ending in {suffix}"))
}

#[test]
fn false_and_chain_folds_to_literal() {
    // lobsters-bench disables page caching as `false && <unresolvable
    // tail>`; the tail is provably dead and must not survive to AOT.
    let app = app_with(
        "",
        r#"  CACHE_PAGE = proc { false && @user.blank? && missing_helper_nobody_defines }
  def index
    render plain: "ok"
  end
"#,
    );
    let src = emitted(&ruby::emit_lowered_controllers(&app), "home_controller.rb");
    assert!(src.contains("CACHE_PAGE = proc { false }"), "{src}");
    assert!(!src.contains("missing_helper_nobody_defines"), "{src}");
}

#[test]
fn values_at_splat_desugars_to_map() {
    let app = app_with(
        "",
        r#"  def index
    by_id = { 1 => "a" }
    ids = [1, 2]
    @rows = by_id.values_at(*ids).compact
    render plain: "ok"
  end
"#,
    );
    let src = emitted(&ruby::emit_lowered_controllers(&app), "home_controller.rb");
    assert!(src.contains("ids.map { |__k| by_id[__k] }.compact"), "{src}");
    assert!(!src.contains("values_at"), "{src}");
}

#[test]
fn scalar_body_backfills_rbs_return() {
    // `-> untyped` on a trivially-String class method turned a String
    // slice into a whole-program poly dispatch under spinel
    // (User.username_regex_s[1...-1]). The backfill pins the scalar.
    let app = app_with(
        r#"  def self.username_regex_s
    "/^" + "x" + "$/"
  end
  def dom_suffix
    "user-" + "row"
  end
  def maybe_name
    return nil if @username.nil?
    @username
  end
"#,
        "  def index\n    render plain: \"ok\"\n  end\n",
    );
    let files = ruby::emit_lowered_models(&app);
    let rbs = emitted(&files, "user.rbs");
    assert!(rbs.contains("def self.username_regex_s: () -> String"), "{rbs}");
    assert!(rbs.contains("def dom_suffix: () -> String"), "{rbs}");
    // An early `return` of another shape must NOT be pinned over.
    assert!(rbs.contains("def maybe_name: () -> untyped"), "{rbs}");
}

#[test]
fn assignment_receiver_keeps_parens() {
    // `(rd = session[:redirect_to]).present?` rendered bare re-parses
    // as `rd = <call>.present?` — the local silently becomes a bool
    // (lobsters login handed it to redirect_to).
    let app = app_with(
        "",
        r#"  def index
    if (rd = session[:redirect_to]).present?
      return redirect_to(rd)
    end
    render plain: "ok"
  end
"#,
    );
    let src = emitted(&ruby::emit_lowered_controllers(&app), "home_controller.rb");
    assert!(
        src.contains("(rd = session[:redirect_to])"),
        "assignment receiver must keep its parens:\n{src}"
    );
}

#[test]
fn relation_scope_delegates_are_exact_arity_and_skip_conflicts() {
    // The blanket `(*args, **kwargs)` forward didn't survive spinel's C
    // stage (splat through the class-value dispatch) and hid arity
    // errors; delegates now carry each scope's own params. `recent` is
    // declared with two shapes (zero-arg vs two optionals) — no single
    // def can forward both, so it gets no delegate, ledgered in the
    // header.
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  scope :active, -> { where("banned_at is null") }
  scope :low_scoring, ->(max = 5) { where("karma < ?", max) }
  scope :recent, -> { order("id desc") }
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
  scope :recent, ->(user = nil, exclude_tags = nil) { order("id desc") }
end
"#,
        ),
    ]))
    .expect("ingest");
    let files = ruby::emit_spinel(&app);
    let src = emitted(&files, "relation_scopes.rb");
    assert!(src.contains("def active\n      klass.active(self)"), "{src}");
    assert!(
        src.contains("def low_scoring(max = 5)\n      klass.low_scoring(max, self)"),
        "{src}"
    );
    assert!(!src.contains("def recent"), "conflicting shapes get no delegate:\n{src}");
    assert!(src.contains("NoMethodError: recent"), "skip must be ledgered:\n{src}");
    assert!(!src.contains("*args"), "no splat forwarding:\n{src}");
}
