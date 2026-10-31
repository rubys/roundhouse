//! PostgreSQL: pluck qualified and transaction actions, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE groups (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT groups_pkey PRIMARY KEY (id)
);

CREATE TABLE members (
    id bigserial NOT NULL,
    group_id bigint,
    label character varying,
    score integer,
    CONSTRAINT members_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_pluck_qualified
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const GROUP: &str = r#"class Group < ApplicationRecord
  has_many :members
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  belongs_to :group

  after_commit :note_actions

  def self.noted
    @noted ||= []
  end

  def note_actions
    self.class.noted << [:create, :update, :destroy].select { |action| transaction_include_any_action?([action]) }
  end
end
"#;

const PROBE: &str = r##"class QualifiedProbe
  def self.lines
    out = []
    group = Group.create!(name: "g")
    Member.create!(group_id: group.id, label: "a", score: 1)
    Member.create!(group_id: group.id, label: "b", score: nil)
    join = "INNER JOIN groups ON groups.id = members.group_id"
    rows = Member.joins(join).where(groups: { name: "g" }).order(:id).pluck("members.label", "members.score", "groups.name")
    out << "joined: #{rows.inspect}"
    out << "one: #{Member.joins(join).order(:id).pluck("members.label").inspect}"
    out << "raw: #{Member.order(:id).pluck(Arel.sql("upper(label)")).inspect}"
    out << "plain: #{Member.order(:id).pluck(:label, "score").inspect}"
    # repository plucks `Arel.sql("max(x) AS y")`).
    out << "aliased: #{Member.order(:id).pluck(Arel.sql("upper(label) AS shout"), :score).inspect}"
    out << "aliased quoted: #{Member.group(:label).order(:label).pluck(:label, Arel.sql(%q(count(*) AS "total"))).inspect}"
    # `select(:sym)` of a column the model does not have is the bare
    # column of the joined table, not `members.name`.
    out << "joined select: #{Member.joins(join).order(:id).select(:name).map { |m| m[:name] }.inspect}"
    rel = Member.order(:id)
    out << "loaded: #{rel.loaded? ? true : false} #{rel.load.loaded?}"
    member = Member.first
    member.update!(label: "c")
    member.destroy!
    out << "actions: #{Member.noted.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    r#"joined: [["a", 1, "g"], ["b", nil, "g"]]"#,
    r#"one: ["a", "b"]"#,
    r#"raw: ["A", "B"]"#,
    r#"plain: [["a", 1], ["b", nil]]"#,
    r#"aliased: [["A", 1], ["B", nil]]"#,
    r#"aliased quoted: [["a", 1], ["b", 1]]"#,
    r#"joined select: ["g", "g"]"#,
    "loaded: false true",
    r#"actions: [[:create], [:create], [:update], [:destroy]]"#,
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/group",
    "app/models/member",
    "app/models/qualified_probe",
    "app/models/qualified_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/group.rb", GROUP),
    ("app/models/member.rb", MEMBER),
    ("app/models/qualified_probe.rb", PROBE),
];

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn qualified_pluck_and_transaction_actions_match_native_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "pluck-qualified",
        &FILES,
        &emitted,
        "rh_spec_pluck_qualified",
        "QualifiedProbe",
        &LOADS[..4],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
