//! PostgreSQL: model i18n and rails constants emit, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_probe_i18n_constants\n";
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";
const STRUCTURE: &str = "CREATE TABLE convs (\n    id bigserial NOT NULL,\n    name character varying,\n    CONSTRAINT convs_pkey PRIMARY KEY (id)\n);\n";

const MODEL: &str = r#"class Conv < ApplicationRecord
  PLATFORMS = %i[a b].freeze

  def self.platforms_for_locale(locale)
    PLATFORMS.dup.append(:other).each_with_object({}).each do |platform, hash|
      hash[platform] = I18n.t(platform, scope: "launch.views", locale: locale)
    end.freeze
  end

  def not_authorized
    i18n_key = "metafields.access_control.access_denied"
    { message: I18n.t(i18n_key), i18n_key: i18n_key }
  end

  def self.sanitizer
    @sanitizer ||= Rails::Html::Sanitizer.safe_list_sanitizer.new
  end

  def self.entrypoint
    "x"
  rescue ActionDispatch::MissingController
    "Unavailable"
  end
end
"#;

fn emitted_model() -> (String, Vec<String>) {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/conv.rb", MODEL)
        .emit(roundhouse::project::BuildTarget::Ruby);
    (std::fs::read_to_string(emitted.join("app/models/conv.rb")).expect("emitted model"), errors)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn model_i18n_the_catalog_cannot_fold_runs_against_the_gem() {
    let (text, _) = emitted_model();
    assert!(!text.contains("I18n not supported"), "{text}");
    assert!(text.contains(r#"I18n.t(platform, scope: "launch.views", locale: locale)"#), "{text}");
    assert!(text.contains("message: I18n.t(i18n_key)"), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn sanitizer_and_missing_controller_are_written_as_authored() {
    let (text, errors) = emitted_model();
    assert!(!text.contains("constant not supported"), "{text}");
    assert!(text.contains("Rails::Html::Sanitizer.safe_list_sanitizer.new"), "{text}");
    assert!(text.contains("rescue ActionDispatch::MissingController"), "{text}");
    let refused: Vec<_> = errors.iter().filter(|e| e.contains("constant not supported")).collect();
    assert!(refused.is_empty(), "{refused:?}");
}
