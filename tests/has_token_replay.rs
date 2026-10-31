//! PostgreSQL: has token replay, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const APPLICATION: &str = "require_relative \"boot\"\nrequire \"rails\"\nBundler.require(*Rails.groups)\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"cards\" do |t|\n    t.string \"token\"\n  end\nend\n";
const CARD: &str = "class Card < ApplicationRecord\n  has_token :token\nend\n";

fn emit(gemfile: &str) -> (String, Vec<String>) {
    let (tree, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile", gemfile)
        .write("config.ru", "")
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_has_token_unused\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/card.rb", CARD)
        .emit(BuildTarget::Ruby);
    (std::fs::read_to_string(tree.join("app/models/card.rb")).expect("card.rb"), errors)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn has_token_is_replayed_when_the_gem_is_required() {
    let (card, errors) = emit("source \"https://rubygems.org\"\ngem \"rails\"\ngem \"identifications\"\n");
    assert!(card.contains("has_token(:token)") || card.contains("has_token :token"), "{card}");
    assert!(!errors.iter().any(|e| e.contains("has_token")), "{errors:#?}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn has_token_stays_dropped_without_the_gem() {
    let (card, errors) = emit("source \"https://rubygems.org\"\ngem \"rails\"\n");
    assert!(!card.contains("has_token"), "{card}");
    assert!(errors.iter().any(|e| e.contains("has_token")), "{errors:#?}");
}

const ITEM: &str = "class Item < ApplicationRecord\n  has_handle :handle, scope: :shop_id, sticky: true,\n    limit_similar_handles: ->(record) { record.handle.blank? },\n    utf8: -> do\n      #: self as Item\n      true\n    end\nend\n";

fn emit_item(gemfile: &str) -> (String, Vec<String>) {
    let (tree, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile", gemfile)
        .write("config.ru", "")
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_has_token_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"items\" do |t|\n    t.string \"handle\"\n    t.string \"title\"\n    t.integer \"shop_id\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/item.rb", ITEM)
        .emit(BuildTarget::Ruby);
    (std::fs::read_to_string(tree.join("app/models/item.rb")).expect("item.rb"), errors)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn has_handle_is_replayed_when_the_gem_is_required() {
    let (item, errors) = emit_item("source \"https://rubygems.org\"\ngem \"rails\"\ngem \"identifications\"\n");
    assert!(item.contains("has_handle"), "{item}");
    assert!(!errors.iter().any(|e| e.contains("has_handle")), "{errors:#?}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn has_handle_stays_dropped_without_the_gem() {
    let (item, errors) = emit_item("source \"https://rubygems.org\"\ngem \"rails\"\n");
    assert!(!item.contains("has_handle"), "{item}");
    assert!(errors.iter().any(|e| e.contains("has_handle")), "{errors:#?}");
}
