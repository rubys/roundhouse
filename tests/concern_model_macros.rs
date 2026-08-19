//! Model-DSL MACROS declared inside a concern's `included do` reach the
//! models that include it.
//!
//! `has_many` and friends already did — the classifier gives them a
//! `ModelBodyItem` variant and the splice carries every one. The macros
//! that never got a variant (`has_one_attached`, `has_rich_text`,
//! `has_secure_token`, …) land in the `Unknown` holding pen instead,
//! and the splice kept only the block-form callbacks out of it. So
//! campfire's `Message::Attachment`, whose entire `included do` is
//! `has_one_attached :attachment`, contributed nothing: `Message#
//! attachment` was never synthesized, and the concern's own
//! `attachment?` — which calls it — emitted right beside the hole.
//!
//! The BLOCK form is the one campfire writes (`do |attachable|
//! attachable.variant :thumb, … end`). Variants aren't modeled; the
//! attachment-existence half still has to expand.

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

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#,
        ),
        (
            "app/models/message/attachment.rb",
            r#"module Message::Attachment
  extend ActiveSupport::Concern

  THUMBNAIL_MAX_WIDTH = 1200

  included do
    has_one_attached :attachment do |attachable|
      attachable.variant :thumb, resize_to_limit: [ THUMBNAIL_MAX_WIDTH, 800 ]
    end
  end

  def attachment?
    attachment.attached?
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Attachment
end
"#,
        ),
    ]))
    .expect("ingest concern-macro app")
}

fn model_src(name: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

/// The reader the concern declares lands on the INCLUDER, scoped to the
/// includer's own record type — not on the module, which has no table.
#[test]
fn a_concerns_has_one_attached_reaches_the_includer() {
    let src = model_src("message.rb");
    assert!(
        src.contains(r#"ActiveStorage::Attached.new("Message", @id, "attachment")"#),
        "the concern's has_one_attached must synthesize on Message:\n{src}"
    );
}
