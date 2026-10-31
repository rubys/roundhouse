//! PostgreSQL: model alias method attribute, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE fees (
    id bigserial NOT NULL,
    amount numeric,
    credit_card_flat_rate numeric,
    CONSTRAINT fees_pkey PRIMARY KEY (id)
);
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_alias_method_attribute\n";

const FEE: &str = r#"class Fee < ApplicationRecord
  unsafe_self = self #: as untyped
  unsafe_self.alias_method(:amount_before_tax, :amount)
  unsafe_self.alias_method(:amount_before_tax=, :amount=)
  alias_method :flat_fee, :credit_card_flat_rate
  alias_method :flat_fee=, :credit_card_flat_rate=

  def label
    "fee"
  end
  alias_method :caption, :label
end
"#;

fn emit_fee() -> String {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/fee.rb", FEE)
        .emit(BuildTarget::Ruby);
    let dropped: Vec<&String> = errors.iter().filter(|e| e.contains("alias_method")).collect();
    assert!(dropped.is_empty(), "{dropped:#?}");
    std::fs::read_to_string(emitted.join("app/models/fee.rb")).unwrap()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn alias_method_of_a_column_reader_is_replayed_in_the_class_body() {
    let text = emit_fee();
    for (guard, alias) in [
        ("method_defined?(:amount)", "unsafe_self.alias_method(:amount_before_tax, :amount)"),
        ("method_defined?(:amount=)", "unsafe_self.alias_method(:amount_before_tax=, :amount=)"),
        ("method_defined?(:credit_card_flat_rate)", "alias_method :flat_fee, :credit_card_flat_rate"),
        ("method_defined?(:credit_card_flat_rate=)", "alias_method :flat_fee=, :credit_card_flat_rate="),
    ] {
        let replayed = format!("if {guard} || private_{guard}\n    {alias}\n  else\n    STDERR.puts(");
        assert!(text.contains(&replayed), "`{alias}` is not replayed under its guard\n{text}");
    }
    assert!(text.contains("  def caption\n"), "a def's alias is still copied\n{text}");
}

#[test]
fn replayed_alias_method_parses_without_a_warning() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/fee.rb", FEE)
        .emit(BuildTarget::Ruby);
    let path = emitted.join("app/models/fee.rb");
    let out = std::process::Command::new("ruby").arg("-wc").arg(&path).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && !stderr.contains("warning"), "{stderr}");
}

/// A library class below an ActiveModel class the ingest lowers as a
/// tableless model takes the STI subclass path, whose body statements are
/// emitted as written, unguarded; its alias target may come from a mixin
/// macro the emit drops (openid_connect's `IdToken < ConnectObject`:
/// `alias_method :subject, :sub` after `attr_required :sub`). Replaying it
/// raised NameError at boot for the whole v2-r48 tree; it stays dropped
/// there, with its warning.
#[test]
fn an_sti_subclass_alias_method_is_not_replayed_unguarded() {
    let attr_required = "module AttrRequired\n  def self.included(base)\n    base.extend(ClassMethods)\n  end\n\n  module ClassMethods\n    def attr_required(*names)\n      names.each { |n| attr_accessor(n) }\n    end\n  end\nend\n";
    let base = "class ConnectObject\n  include ActiveModel::Validations, AttrRequired\n\n  attr_accessor :raw_attributes\n\n  def initialize(attributes = {})\n    self.raw_attributes = attributes\n  end\nend\n";
    let sub = "class IdToken < ConnectObject\n  attr_required :sub\n  alias_method :subject, :sub\n\n  def verify!\n    subject\n  end\nend\n";
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/services/attr_required.rb", attr_required)
        .write("app/services/connect_object.rb", base)
        .write("app/services/id_token.rb", sub)
        .emit(BuildTarget::Ruby);
    fn find(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path, name) {
                    return Some(found);
                }
            } else if path.file_name().is_some_and(|f| f == name) && !path.to_string_lossy().contains("/sig/") {
                return Some(path);
            }
        }
        None
    }
    let path = find(&emitted, "id_token.rb").expect("id_token.rb emitted");
    let text = std::fs::read_to_string(&path).unwrap();
    let unguarded = text.lines().any(|l| l.trim() == "alias_method :subject, :sub") && !text.contains("method_defined?(:sub)");
    assert!(!unguarded, "an STI subclass's alias_method is written unguarded\n{text}");
}
