#![allow(dead_code)]

pub const BASE: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";
pub const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :pty_sessions do |t|
    t.string :driver
    t.string :approval_policy
  end
end
"#;
pub const PROVIDER: &str = r#"class PtySession < ApplicationRecord
  enum :driver, { terminal: "terminal", protocol: "protocol" }, prefix: true, validate: true
  enum :approval_policy, { bypass: "bypass", ask: "ask" }, prefix: true, validate: true
end
"#;
pub const FORM: &str = r#"class SessionStart
  include ActiveModel::Model
  include ActiveModel::Attributes

  APPROVAL_POLICIES = PtySession.approval_policies.keys.freeze
  attribute :driver, :string, default: "terminal"
  attribute :approval_policy, :string, default: "bypass"

  validates :driver, inclusion: { in: PtySession.drivers.keys }
  validates :approval_policy, inclusion: { in: APPROVAL_POLICIES }
end
"#;

pub fn overlay(form: &str) -> super::emit_and_run::Overlay {
    super::emit_and_run::empty_app()
        .write("app/models/application_record.rb", BASE)
        .write("app/models/pty_session.rb", PROVIDER)
        .write("app/models/session_start.rb", form)
        .write("db/schema.rb", SCHEMA)
}

pub const ASSERTIONS: &str = r#"
[["terminal", "bypass", []], ["protocol", "ask", []],
 ["bogus", "bypass", ["Driver is not included in the list"]],
 ["terminal", "bogus", ["Approval policy is not included in the list"]],
 [nil, nil, ["Driver is not included in the list", "Approval policy is not included in the list"]],
 ["", "", ["Driver is not included in the list", "Approval policy is not included in the list"]]].each do |driver, policy, expected|
  form = SessionStart.new(driver: driver, approval_policy: policy)
  valid = form.valid?
  raise "valid? differs" unless valid == expected.empty?
  raise "validation errors differ" unless messages(form) == expected
end
puts "external enum inclusion contract passed"
"#;
