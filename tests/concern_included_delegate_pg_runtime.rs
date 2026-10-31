//! PostgreSQL: concern included delegate, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE members (
    id bigserial NOT NULL,
    email character varying,
    CONSTRAINT members_pkey PRIMARY KEY (id)
);
CREATE TABLE badges (
    id bigserial NOT NULL,
    member_id bigint,
    owned boolean,
    label character varying,
    CONSTRAINT badges_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_concern_included_delegate
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const BADGED: &str = r#"module Badged
  extend ActiveSupport::Concern

  included do
    has_one :badge, inverse_of: :member
    delegate :owned, :owned?, to: :badge, allow_nil: true
    delegate :label, to: :badge, prefix: true, allow_nil: true
    validates :email, uniqueness: true, unless: [:owned?]
  end
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  include Badged
end
"#;

const BADGE: &str = r#"class Badge < ApplicationRecord
  belongs_to :member, inverse_of: :badge
end
"#;

const PROBE: &str = r##"class DelegateProbe
  def self.lines
    out = []
    Member.create!(email: "a@x")
    bare = Member.new(email: "a@x")
    out << "bare: #{bare.owned.inspect} #{bare.owned?.inspect} #{bare.badge_label.inspect} #{bare.valid?} #{bare.errors.full_messages.inspect}"
    dup = Member.new(email: "a@x")
    dup.save!(validate: false)
    Badge.create!(member_id: dup.id, owned: true, label: "gold")
    owned = Member.find(dup.id)
    out << "owned: #{owned.owned.inspect} #{owned.owned?.inspect} #{owned.badge_label.inspect} #{owned.valid?}"
    out << "public: #{Member.public_method_defined?(:owned?)} #{Member.instance_method(:owned?).owner}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    r#"bare: nil nil nil false ["Email has already been taken"]"#,
    r#"owned: true true "gold" true"#,
    r#"public: true Member"#,
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/badged",
    "app/models/member",
    "app/models/badge",
    "app/models/delegate_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/badged.rb", BADGED),
    ("app/models/member.rb", MEMBER),
    ("app/models/badge.rb", BADGE),
    ("app/models/delegate_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The includer gets every forwarder the block's declarations define,
/// the `?` predicate and the prefixed name included.
#[test]
#[ignore = "fork spec: postgresql"]
fn an_included_block_delegate_defines_its_forwarders_on_the_includer() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let member = std::fs::read_to_string(emitted.join("app/models/member.rb")).unwrap();
    for name in ["def owned(...)\n", "def owned?(...)\n", "def badge_label(...)\n"] {
        assert!(member.contains(name), "missing {name:?} in:\n{member}");
    }
    assert!(!member.contains("delegate :owned"), "{member}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn included_block_delegates_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "concern-included-delegate",
        &FILES,
        &emitted,
        "rh_spec_concern_included_delegate",
        "DelegateProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
