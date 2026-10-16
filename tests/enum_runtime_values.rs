//! `enum` declarations whose stored values or labels are written as
//! constants rather than literals.
//!
//! Shopify core spells most of its enums this way:
//!
//! ```ruby
//! enum :status, { archived: Status::ARCHIVED, open: Status::OPEN }
//! enum :kind, { ASSOCIATE => ASSOCIATE, POS_USER => POS_USER }
//! enum :phase, { "#{UNSET_STATUS}": 0, "#{PENDING_STATUS}": 1 }
//! enum :mode, MODES.to_h { |m| [m, m.to_s] }
//! ```
//!
//! Rails evaluates the mapping when the class body runs and defines a
//! scope, a predicate and a bang writer per label. The labels of these
//! are all knowable at ingest (a symbol key, or a constant the class body
//! assigned above); the stored values may be a constant defined
//! elsewhere. Ingest used to refuse every one, dropping the declaration,
//! and with it every `record.archived?` / `Model.archived` call site.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "accounts" do |t|
    t.string "status"
    t.string "kind"
    t.integer "phase"
    t.string "mode"
    t.string "state"
  end
end
"#;

fn account(body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n".to_string()),
        ("app/models/account.rb", format!("class Account < ApplicationRecord\n{body}\nend\n")),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("an enum with constant parts must ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel)
        .expect("spinel files")
        .into_iter()
        .find(|(p, _)| p == "app/models/account.rb")
        .map(|(_, c)| c)
        .expect("account.rb")
}

/// The stored value is a constant only the running program can read; the
/// generated predicate and scope compare against it, unfolded.
#[test]
fn a_constant_stored_value_is_compared_unfolded() {
    let out = account("  enum :status, { archived: Status::ARCHIVED, open: Status::OPEN }");
    assert!(out.contains("def archived?"), "got:\n{out}");
    assert!(out.contains("status == Status::ARCHIVED"), "got:\n{out}");
    assert!(out.contains("def open!"), "got:\n{out}");
    assert!(out.contains("update!({ status: Status::OPEN })"), "got:\n{out}");
}

/// A label spelled as a constant the class body assigned, including an
/// alias of another constant, names the method it holds.
#[test]
fn a_constant_label_names_its_string() {
    let out = account(
        "  POS_USER = \"pos_user\"\n  ASSOCIATE = POS_USER\n  enum :kind, { ASSOCIATE => ASSOCIATE, POS_USER => POS_USER }",
    );
    assert!(out.contains("def pos_user?"), "got:\n{out}");
    assert!(out.contains("kind == \"pos_user\""), "got:\n{out}");
}

/// `"#{UNSET}": 0` is a symbol whose name interpolates class constants.
#[test]
fn an_interpolated_label_is_folded() {
    let out = account(
        "  UNSET = \"unset\"\n  READY = \"ready\"\n  enum :phase, { \"#{UNSET}\": 0, \"#{READY}\": 1 }",
    );
    assert!(out.contains("def unset?"), "got:\n{out}");
    assert!(out.contains("def ready!"), "got:\n{out}");
    assert!(out.contains("update!({ phase: 1 })"), "got:\n{out}");
}

/// A mapping derived from a class-body label list maps each label to its
/// own string.
#[test]
fn an_identity_mapping_over_a_constant_list_expands() {
    let out = account("  MODES = %i[fast slow].freeze\n  enum :mode, MODES.to_h { |m| [m, m.to_s] }, prefix: :mode");
    assert!(out.contains("def mode_fast?"), "got:\n{out}");
    assert!(out.contains("mode == \"slow\""), "got:\n{out}");
}

/// `scopes: false` and `instance_methods: false` turn each family off.
#[test]
fn scopes_and_instance_methods_can_be_disabled() {
    let out = account("  STATES = %w[on off]\n  enum :state, STATES.index_by(&:to_s), scopes: false, instance_methods: false");
    assert!(!out.contains("def on"), "got:\n{out}");
    assert!(!out.contains("def off"), "got:\n{out}");
}

/// `["draft", "active"].index_by(&:to_sym)` keys each string by its
/// symbol and stores the string.
#[test]
fn indexing_a_string_list_by_symbol_maps_each_label_to_its_string() {
    let out = account("  enum :status, [\"draft\", \"active\"].index_by(&:to_sym), default: :draft");
    assert!(out.contains("def draft?"), "got:\n{out}");
    assert!(out.contains("status == \"active\""), "got:\n{out}");
}
