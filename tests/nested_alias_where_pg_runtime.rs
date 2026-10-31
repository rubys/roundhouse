//! PostgreSQL: nested alias where, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE groups (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT groups_pkey PRIMARY KEY (id)
);

CREATE TABLE members (
    id bigserial NOT NULL,
    group_id bigint,
    CONSTRAINT members_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  database: rh_spec_nested_alias_where\n";
const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const GROUP: &str = "class Group < ApplicationRecord\n  has_many :members\nend\n";
const MEMBER: &str = "class Member < ApplicationRecord\n  belongs_to :group\n  alias_attribute :cohort_id, :group_id\nend\n";

const PROBE: &str = r##"class NestedAliasProbe
  def self.lines
    group = Group.create!(name: "g")
    Group.create!(name: "other")
    Member.create!(group_id: group.id)
    [
      "nested: #{Group.joins(:members).where(members: { cohort_id: group.id }).pluck(:name).inspect}",
      "negated: #{Group.joins(:members).where.not(members: { cohort_id: group.id }).count}",
    ]
  end
end
"##;

const EXPECTED: [&str; 2] = [r#"nested: ["g"]"#, "negated: 0"];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/group",
    "app/models/member",
    "app/models/nested_alias_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/group.rb", GROUP),
    ("app/models/member.rb", MEMBER),
    ("app/models/nested_alias_probe.rb", PROBE),
    
];

/// A nested where hash over an association's table reads that model's
/// `alias_attribute` column (`where(members: {cohort_id:})` is
/// `members.group_id`), natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_nested_where_hash_names_the_owner_tables_alias_column() {
    let app = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = app.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "nested-alias-where",
        &FILES,
        &emitted,
        "rh_spec_nested_alias_where",
        "NestedAliasProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
