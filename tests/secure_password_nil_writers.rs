//! A supplied nil reaches a secure-password virtual writer; an omitted key does not.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "password_digest"
    t.string "display_name"
  end
end
"#)
        .write("app/models/user.rb", r#"class User < ApplicationRecord
  has_secure_password validations: false, reset_token: false
  attr_accessor :nickname

  def password=(value)
    @password_supplied = true
    @password = value
  end

  def password_confirmation=(value)
    @confirmation_supplied = true
    @password_confirmation = value
  end

  def password_supplied?
    @password_supplied == true
  end

  def confirmation_supplied?
    @confirmation_supplied == true
  end

  def reset_supplied
    @password_supplied = false
    @confirmation_supplied = false
  end
end
"#)
}

fn writer_flags(field: &str) -> (&str, &str, &str, &str) {
    match field {
        "password" => ("password_supplied?", "confirmation_supplied?", "password_confirmation", "confirmation"),
        "password_confirmation" => ("confirmation_supplied?", "password_supplied?", "password", "secret"),
        _ => unreachable!(),
    }
}

fn initialization_assertions(field: &str) -> String {
    let (supplied, omitted, _, _) = writer_flags(field);
    format!(r#"
user = User.new
raise "new dispatched omitted keys" if user.password_supplied? || user.confirmation_supplied?
user = User.new({field}: nil)
raise "new skipped {field}: nil" unless user.{supplied} && user.{field}.nil?
raise "new dispatched omitted writer" if user.{omitted}
"#)
}

fn update_assertions(method: &str, field: &str) -> String {
    let (supplied, omitted, other_field, other_value) = writer_flags(field);
    format!(r#"
user = User.new(password: "secret", password_confirmation: "confirmation")
raise "new lost supplied values" unless user.password == "secret" && user.password_confirmation == "confirmation"
user.reset_supplied
user.{method}({{}})
raise "{method} dispatched omitted keys" if user.password_supplied? || user.confirmation_supplied?
raise "{method} changed omitted values" unless user.password == "secret" && user.password_confirmation == "confirmation"
user.{method}({field}: nil)
raise "{method} skipped {field}: nil" unless user.{supplied} && user.{field}.nil?
raise "{method} dispatched omitted writer" if user.{omitted}
raise "{method} changed omitted value" unless user.{other_field} == "{other_value}"
user.reset_supplied
user.{method}(password: "changed", password_confirmation: "new confirmation")
raise "{method} skipped supplied values" unless user.password_supplied? && user.confirmation_supplied?
raise "{method} lost supplied values" unless user.password == "changed" && user.password_confirmation == "new confirmation"
"#)
}

// This fix changes only secure-password dispatch. Other virtual writers
// retain their existing nil guard; nullable columns still accept nil.
fn unrelated_nil_assertions() -> String {
    ["update", "update!"].into_iter().map(|method| format!(r#"
user = User.new(display_name: "name")
user.nickname = "nickname"
user.{method}(nickname: nil, display_name: nil)
raise "{method} changed unrelated virtual nil handling" unless user.nickname == "nickname"
raise "{method} lost nullable column assignment" unless user.display_name.nil?
"#)).collect::<Vec<_>>().join("\n")
}

fn run_spinel(assertions: &str) {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{assertions}",
    );
    app().run_spinel(&script).assert_passes();
}

#[test]
fn supplied_nil_password_initialization_runs_on_ruby() {
    app().run_ruby(&initialization_assertions("password")).assert_passes();
}

#[test]
fn supplied_nil_confirmation_initialization_runs_on_ruby() {
    app().run_ruby(&initialization_assertions("password_confirmation")).assert_passes();
}

#[test]
fn supplied_nil_password_update_runs_on_ruby() {
    app().run_ruby(&update_assertions("update", "password")).assert_passes();
}

#[test]
fn supplied_nil_confirmation_update_runs_on_ruby() {
    app().run_ruby(&update_assertions("update", "password_confirmation")).assert_passes();
}

#[test]
fn supplied_nil_password_update_bang_runs_on_ruby() {
    app().run_ruby(&update_assertions("update!", "password")).assert_passes();
}

#[test]
fn supplied_nil_confirmation_update_bang_runs_on_ruby() {
    app().run_ruby(&update_assertions("update!", "password_confirmation")).assert_passes();
}

#[test]
fn unrelated_nil_handling_stays_unchanged_on_ruby() {
    app().run_ruby(&unrelated_nil_assertions()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_password_initialization_runs_on_spinel() {
    run_spinel(&initialization_assertions("password"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_confirmation_initialization_runs_on_spinel() {
    run_spinel(&initialization_assertions("password_confirmation"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_password_update_runs_on_spinel() {
    run_spinel(&update_assertions("update", "password"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_confirmation_update_runs_on_spinel() {
    run_spinel(&update_assertions("update", "password_confirmation"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_password_update_bang_runs_on_spinel() {
    run_spinel(&update_assertions("update!", "password"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn supplied_nil_confirmation_update_bang_runs_on_spinel() {
    run_spinel(&update_assertions("update!", "password_confirmation"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn unrelated_nil_handling_stays_unchanged_on_spinel() {
    run_spinel(&unrelated_nil_assertions());
}
