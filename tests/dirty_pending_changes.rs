//! The BEFORE-save half of ActiveModel::Dirty: `<col>_changed?`,
//! `will_save_change_to_<col>?` and `<col>_was`.
//!
//! The analyzer has always registered these names on every model, but
//! nothing synthesized them, so a call type-checked and then raised
//! NoMethodError. lobsters' User guards `validate_username_timeouts` on
//! `username_changed?`, which runs on every User save and stopped 183
//! of its 359 model specs. campfire's Room reads `type_changed?` and
//! `type_was`.
//!
//! Each reader delegates to a name-taking runtime Base method
//! (`attribute_changed?` / `attribute_was`) over `changes_to_save`,
//! the same split the saved-change family takes: the real diff lives in
//! the ruby-family connection.rb reopen, and Base's stubs answer
//! false/nil on the strict lanes.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit_user(model_src: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"username\", null: false\n    t.string \"email\", null: false\n    t.string \"about\"\n  end\nend\n".to_vec(),
        ),
        (PathBuf::from("app/models/user.rb"), model_src.as_bytes().to_vec()),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("user.rb"))
        .expect("no user.rb emitted")
        .content
        .clone()
}

/// A name a model body sends is synthesized, delegating with a STRING
/// key: the diff is over `attributes`, keyed by column-name String.
#[test]
fn a_sent_name_is_synthesized_and_delegates() {
    let src = emit_user(
        "class User < ApplicationRecord\n  validate :timeouts\n\n  def timeouts\n    return unless username_changed?\n    errors.add(:username, \"was #{username_was}\")\n  end\nend\n",
    );
    assert!(src.contains("def username_changed?"), "{src}");
    assert!(src.contains(r#"attribute_changed?("username")"#), "{src}");
    assert!(src.contains("def username_was"), "{src}");
    assert!(src.contains(r#"attribute_was("username")"#), "{src}");
}

/// The Symbol form counts as demand: `before_save :x, if:
/// :will_save_change_to_email?` names the predicate without sending it.
#[test]
fn a_symbol_condition_is_demand() {
    let src = emit_user(
        "class User < ApplicationRecord\n  before_save :reset_token, if: :will_save_change_to_email?\n\n  def reset_token\n  end\nend\n",
    );
    assert!(src.contains("def will_save_change_to_email?"), "{src}");
    assert!(src.contains(r#"attribute_changed?("email")"#), "{src}");
}

/// DEMAND-GATED: nothing names `about_changed?`, so it is not
/// synthesized. Three methods per column on every model would be
/// surface every strict emitter must learn (rust's model impl, go's
/// call-form list), for an answer that is constant there.
#[test]
fn an_unnamed_column_gets_no_pending_readers() {
    let src = emit_user(
        "class User < ApplicationRecord\n  def check\n    username_changed?\n  end\nend\n",
    );
    for name in ["about_changed?", "about_was", "will_save_change_to_about?", "email_changed?"] {
        assert!(!src.contains(&format!("def {name}")), "{name} synthesized without demand:\n{src}");
    }
}

/// A model's own definition wins; a synthesized method of the same name
/// would have dropped it.
#[test]
fn a_model_definition_wins() {
    let src = emit_user(
        "class User < ApplicationRecord\n  def username_was\n    \"mine\"\n  end\n\n  def check\n    username_was\n  end\nend\n",
    );
    assert!(src.contains("\"mine\""), "{src}");
    assert!(!src.contains(r#"attribute_was("username")"#), "{src}");
}
