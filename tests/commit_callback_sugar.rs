//! `after_create_commit :method` — the SYMBOL form of Rails' per-lifecycle
//! commit-callback sugar.
//!
//! Only the BLOCK form was ever recognized (`BLOCK_CALLBACK_HOOKS`); the
//! symbol form fell through to `Unknown`, so the declaration was DROPPED.
//! The target method still emitted, and nothing ever called it —
//! campfire's `User` never granted membership to the open rooms, and
//! `Message::Searchable` never wrote a row to its search index. Both
//! failed silently, which is what kept it hidden.
//!
//! `after_save_commit` gets its own hook rather than an `on:` because it
//! spans TWO lifecycle events; mapping it onto the bare `after_commit`
//! would also fire it on destroy, the one event Rails excludes.

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

fn thing_src() -> String {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "things", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        (
            "app/models/thing.rb",
            r#"class Thing < ApplicationRecord
  after_create_commit  :on_create
  after_update_commit  :on_update
  after_destroy_commit :on_destroy
  after_save_commit    :on_save

  def on_create; end
  def on_update; end
  def on_destroy; end
  def on_save; end
end
"#,
        ),
    ]))
    .expect("ingest callback app");
    let files = ruby::emit_lowered_models(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("thing.rb"))
        .map(|f| f.content.clone())
        .expect("thing.rb")
}

/// Each spelling overrides the runtime hook of the SAME name, which
/// `save_after_validation` / `destroy` already fire in Rails' order.
#[test]
fn each_sugar_spelling_overrides_its_own_hook() {
    let src = thing_src();
    for (hook, target) in [
        ("after_create_commit", "on_create"),
        ("after_update_commit", "on_update"),
        ("after_destroy_commit", "on_destroy"),
        ("after_save_commit", "on_save"),
    ] {
        let needle = format!("def {hook}");
        assert!(src.contains(&needle), "expected `{needle}`:\n{src}");
        let idx = src.find(&needle).unwrap();
        let end = (idx + 120).min(src.len());
        assert!(
            src[idx..end].contains(target),
            "`{hook}` must call `{target}`:\n{}",
            &src[idx..end]
        );
    }
}

/// `after_save_commit` must NOT collapse onto the bare `after_commit`
/// hook — the runtime fires that one on destroy too.
#[test]
fn after_save_commit_does_not_become_after_commit() {
    let src = thing_src();
    assert!(
        !src.contains("def after_commit"),
        "no bare after_commit override was declared:\n{src}"
    );
}

fn lambda_thing_src(decl: &str) -> String {
    let model = format!(
        "class Thing < ApplicationRecord\n  {decl}\n\n  def on_create; end\nend\n"
    );
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "things", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        ("app/models/thing.rb", &model),
    ]))
    .expect("ingest callback app");
    let files = ruby::emit_lowered_models(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("thing.rb"))
        .map(|f| f.content.clone())
        .expect("thing.rb")
}

/// Rails means the same thing by a BLOCK and a zero-arity LAMBDA
/// ARGUMENT, and campfire's `Message` writes the second
/// (`after_create_commit -> { room.receive(self) }`). Matching only the
/// block form dropped the declaration silently.
#[test]
fn a_zero_arity_lambda_argument_is_the_block_form() {
    let src = lambda_thing_src("after_create_commit -> { on_create }");
    assert!(
        src.contains("def after_create_commit"),
        "the lambda-argument spelling must fold:\n{src}"
    );
    let idx = src.find("def after_create_commit").unwrap();
    let end = (idx + 120).min(src.len());
    assert!(
        src[idx..end].contains("on_create"),
        "the hook must call the lambda's body:\n{}",
        &src[idx..end]
    );
}

/// A lambda WITH PARAMETERS declines: Rails `instance_exec`s the
/// zero-arity form (so `self` is the record) but passes the record as
/// an ARGUMENT to this one and leaves `self` as the declaring context.
/// Splicing its body into a hook method would bind `self` wrong.
#[test]
fn a_lambda_with_parameters_still_declines() {
    let src = lambda_thing_src("after_create_commit ->(record) { record.on_create }");
    assert!(
        !src.contains("def after_create_commit"),
        "an arity-1 lambda must not fold:\n{src}"
    );
}
