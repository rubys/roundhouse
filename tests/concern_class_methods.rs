//! A model concern's CLASS-side methods reach the models that include it
//! (`splice_concern_class_methods_into_models`).
//!
//! `include` never carries them. ActiveSupport::Concern only gets away
//! with it because `append_features` runs `base.extend ClassMethods`, and
//! the emitted modules have no Concern — so `Message.create_with_attachment!`
//! resolved in analyze (the registry fold copies the class side onto
//! includers) and NoMethodError'd at runtime.
//!
//! The carrier is the whole distinction. `module ClassMethods` and
//! `class_methods do` are inherited; a module's OWN singletons —
//! `module_function :x`, `class << self` — are not, and after ingest's
//! flatten all four look identical.

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
    t.string "token", null: false
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Message::Attachment, Message::Tokenized, Message::Blocklist

  def self.paged?
    true
  end
end
"#,
        ),
        (
            "app/models/message/attachment.rb",
            r#"module Message::Attachment
  extend ActiveSupport::Concern

  MAX_WIDTH = 1200

  module ClassMethods
    def create_with_attachment!(attributes)
      create!(attributes)
    end

    def widest
      MAX_WIDTH
    end

    def paged?
      false
    end
  end
end
"#,
        ),
        (
            "app/models/message/tokenized.rb",
            r#"module Message::Tokenized
  extend ActiveSupport::Concern

  class_methods do
    def from_token(token)
      find_by(token: token)
    end
  end
end
"#,
        ),
        (
            // The lobsters `EmailBlocklistValidation` shape: the module's
            // OWN singleton, which Rails does not put on an includer.
            "app/models/message/blocklist.rb",
            r#"module Message::Blocklist
  extend ActiveSupport::Concern

  def blocked?
    Message::Blocklist.on_blocklist?(body)
  end

  def on_blocklist?(text)
    text == "spam"
  end

  module_function :on_blocklist?
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn message() -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/message.rb"))
        .map(|f| f.content.clone())
        .expect("message.rb")
}

/// The shape that sent me here: campfire's
/// `Message::Attachment::ClassMethods#create_with_attachment!`.
#[test]
fn class_methods_module_reaches_the_including_model() {
    let m = message();
    assert!(
        m.contains("def self.create_with_attachment!(attributes)"),
        "`module ClassMethods` method lands on the model:\n{m}"
    );
}

/// `class_methods do` is sugar Concern turns into that same module, so
/// both spellings have to arrive.
#[test]
fn class_methods_block_reaches_the_including_model() {
    let m = message();
    assert!(
        m.contains("def self.from_token(token)"),
        "`class_methods do` method lands on the model:\n{m}"
    );
}

/// `module_function` makes a singleton on the MODULE. Rails leaves it
/// there. Copying it invented `User.email_on_blocklist?` on three
/// lobsters models before the carrier list existed.
#[test]
fn module_own_singletons_do_not_reach_the_includer() {
    let m = message();
    assert!(
        !m.contains("def self.on_blocklist?"),
        "a module_function singleton stays on its module:\n{m}"
    );
}

/// Ruby's ancestor order: the class's own definition beats the module's.
#[test]
fn the_models_own_class_method_wins() {
    let m = message();
    assert_eq!(
        m.matches("def self.paged?").count(),
        1,
        "exactly one paged?, the model's own:\n{m}"
    );
    assert!(m.contains("    true\n"), "the model's body survives:\n{m}");
}

/// A lifted body's bare constant resolved against the module it was
/// written in and would resolve against the MODEL once moved — the same
/// lexical trap the controller splice hit with lobsters' TIME_INTERVALS.
#[test]
fn a_lifted_body_keeps_its_modules_constants() {
    let m = message();
    assert!(
        m.contains("Message::Attachment::MAX_WIDTH"),
        "bare MAX_WIDTH is qualified to its module:\n{m}"
    );
}
