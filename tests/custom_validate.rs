//! `validate :method_name` — the CUSTOM-validator form.
//!
//! `ValidationRule::Custom` has been in the dialect and in
//! `lower::validations` all along; nothing in ingest ever produced one,
//! and `model_to_library` answered it with an empty vec and the comment
//! "lands when a fixture forces the issue". campfire forced it: with no
//! `validates` and no `belongs_to`, `push_validate_method` saw an empty
//! statement list and returned before synthesizing `validate` — and
//! with no `validate` there is no `valid?` either, which is what all ten
//! `opengraph_metadata_test` cases died on.
//!
//! Rails semantics measured against 8.1: every `validate :sym` runs on
//! every `valid?` unless a `on:` / `if:` / `unless:` option narrows it.
//! Those options are NOT modeled here, and a check that ran
//! unconditionally would reject records Rails accepts — so a call
//! carrying one keeps its pre-existing behaviour (unsupported-DSL
//! ledger) rather than being promoted.

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

fn emitted(model_src: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"url\", null: false\n  end\nend\n",
        ),
        ("app/models/post.rb", model_src),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("post.rb"))
        .expect("no post.rb emitted")
        .content
        .clone()
}

#[test]
fn each_named_method_is_called_from_the_synthesized_validate() {
    let src = emitted(
        r#"class Post < ApplicationRecord
  validate :url_is_present, :url_is_public

  def url_is_present
    errors.add :url, "is blank" if url.blank?
  end

  def url_is_public
    errors.add :url, "is private" if url.start_with?("http://10.")
  end
end
"#,
    );
    let validate = src
        .split("def validate\n")
        .nth(1)
        .expect("no synthesized validate:\n{src}");
    assert!(validate.contains("url_is_present"), "{src}");
    assert!(validate.contains("url_is_public"), "{src}");
}

/// The point of the whole chain: no `validate` meant no `valid?`, and a
/// tableless model has no runtime Base to inherit one from.
#[test]
fn a_tableless_validating_model_gains_valid_and_errors() {
    let src = emitted(
        r#"class Post
  include ActiveModel::Validations

  attr_accessor :url

  validate :url_is_present

  def url_is_present
    errors.add :url, "is blank" if url.blank?
  end
end
"#,
    );
    assert!(src.contains("def valid?"), "{src}");
    assert!(src.contains("def errors"), "{src}");
    assert!(src.contains("def validate"), "{src}");
}

/// `on:` narrows a validation to one persistence context, so the call
/// runs only behind that context's `persisted?` test — never
/// unconditionally, which would reject records Rails accepts. campfire's
/// `Room` writes `validate :direct_rooms_keep_their_type, on: :update`,
/// and its `Session` the `on: :create` twin. A context Rails does not
/// infer from persistence (`on: :publish`) still keeps the call out.
#[test]
fn a_validate_carrying_on_runs_only_in_that_context() {
    let src = emitted(
        r#"class Post < ApplicationRecord
  validate :type_is_unchanged, on: :update
  validate :fresh_only, on: :create
  validate :at_publish, on: :publish

  def type_is_unchanged
    errors.add :url, "changed"
  end

  def fresh_only
    errors.add :url, "fresh"
  end

  def at_publish
    errors.add :url, "publish"
  end
end
"#,
    );
    let synthesized = src.split("def validate\n").nth(1).unwrap_or("");
    let synthesized = synthesized.split("\n  end\n").next().unwrap_or("");
    assert!(synthesized.contains("type_is_unchanged if self.persisted?"), "{src}");
    assert!(synthesized.contains("fresh_only if !(self.persisted?)"), "{src}");
    assert!(!synthesized.contains("at_publish"), "{src}");
}
