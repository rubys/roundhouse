//! A concern's association takes ITS OWNER'S foreign key, not the
//! concern's.
//!
//! `has_one :webhook` inside `User::Bot`'s `included do` defaults its
//! key from the declaring scope — and the declaring scope at ingest is
//! the CONCERN. The key came out `user::bot_id`, the emitted query said
//! `WHERE webhooks.user::bot_id = 5`, and sqlite answered
//! "unrecognized token". Rails derives the key from the class the
//! association ends up on, which is the includer.
//!
//! Only a key still EQUAL to the concern-derived default moves; an
//! explicit `foreign_key:` differs from it and is left as written.

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

fn user_src() -> String {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "webhooks", force: :cascade do |t|
    t.integer "user_id", null: false
  end
  create_table "notes", force: :cascade do |t|
    t.integer "author_id", null: false
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Bot\nend\n",
        ),
        (
            "app/models/user/bot.rb",
            r#"module User::Bot
  extend ActiveSupport::Concern

  included do
    has_one :webhook, dependent: :delete
    has_many :notes, foreign_key: :author_id
  end
end
"#,
        ),
        ("app/models/webhook.rb", "class Webhook < ApplicationRecord\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
    ]))
    .expect("ingest concern-fk app");
    ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("user.rb"))
        .map(|f| f.content)
        .expect("user.rb")
}

/// The defaulted key is the INCLUDER's.
#[test]
fn a_defaulted_key_is_rehomed_to_the_includer() {
    let src = user_src();
    assert!(
        !src.contains("user::bot_id"),
        "the concern's own name must not become a column:\n{src}"
    );
    assert!(src.contains("user_id"), "the includer's key:\n{src}");
}

/// An explicit key is left exactly as written.
#[test]
fn an_explicit_key_is_untouched() {
    let src = user_src();
    assert!(src.contains("author_id"), "explicit foreign_key must survive:\n{src}");
}
