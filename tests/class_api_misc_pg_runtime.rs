//! PostgreSQL: class api misc, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;
const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE groups (
    id bigserial NOT NULL,
    shop_id bigint,
    name character varying,
    number integer,
    active boolean,
    CONSTRAINT groups_pkey PRIMARY KEY (id)
);

CREATE TABLE members (
    id bigserial NOT NULL,
    group_id bigint,
    CONSTRAINT members_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_api
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :groups
  has_one :first_group, class_name: "Group"
  has_many :tags, as: :taggable, class_name: "Member"
  has_one :badge, as: :holder, foreign_type: :holder_kind, class_name: "Member"
  has_many :members, through: :groups
end
"#;

const GROUP: &str = r#"class Group < ApplicationRecord
  belongs_to :shop
  has_many :members
  has_many :members_by_number, class_name: "Member", primary_key: :number, foreign_key: :group_id
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  belongs_to :group
end
"#;

const PROBE: &str = r##"class ClassApiProbe
  def self.lines
    out = []
    shop = Shop.create!(name: "s")
    a = Group.create!(shop_id: shop.id, name: "a", number: 1, active: true)
    Group.create!(shop_id: shop.id, name: "b", number: 2, active: false)
    out << "quoted: #{Group.quoted_table_name} #{Group.quoted_primary_key}"
    out << "scope: #{Group.where("#{Group.quoted_table_name}.#{Group.quoted_primary_key} = ?", a.id).count}"
    out << "keys: #{Shop.reflect_on_association(:groups).active_record_primary_key.inspect} #{Shop.reflect_on_association(:first_group).active_record_primary_key.inspect} #{Group.reflect_on_association(:shop).active_record_primary_key.inspect} #{Shop.reflect_on_association(:members).active_record_primary_key.inspect}"
    out << "type: #{Shop.reflect_on_association(:groups).type.inspect} #{Shop.reflect_on_association(:tags).type.inspect} #{Shop.reflect_on_association(:badge).type.inspect} #{Group.reflect_on_association(:shop).type.inspect}"
    out << "custom: #{Group.reflect_on_association(:members_by_number).active_record_primary_key.inspect}"
    out << "empty where: #{Group.where(id: []).count} #{Group.where(id: []).to_a.size}"
    out << "empty where.not: #{Group.where.not(id: []).count} #{Group.where(shop_id: shop.id).where.not(id: []).count}"
    out << "empty and: #{Group.where(shop_id: shop.id).where(id: []).count} #{Group.where(shop_id: shop.id).where(id: [a.id]).count}"
    out << "empty named bind: #{Group.where("id IN (:ids)", ids: []).count}"
    out << "and: #{Group.and(Group.where(name: "a")).count} #{Group.where(shop_id: shop.id).and(Group.where(number: 2)).pluck(:name).inspect} #{Group.where(name: "a").and(Group.where(name: "b")).count}"
    out << "or: #{Group.or(Group.where(name: "a")).count} #{Group.where(name: "a").or(Group.where(name: "b")).count}"
    out << "empty bind: #{Group.where("id IN (?)", []).count}"
    out
  end
end
"##;

const EXPECTED: [&str; 12] = [
    r#"quoted: "groups" "id""#,
    "scope: 1",
    r#"keys: "id" "id" "id" "id""#,
    r#"type: nil "taggable_type" "holder_kind" nil"#,
    r#"custom: "number""#,
    "empty where: 0 0",
    "empty where.not: 2 2",
    "empty and: 0 1",
    "empty named bind: 0",
    "and: 1 [\"b\"] 0",
    "or: 2 2",
    "empty bind: 0",
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/group",
    "app/models/member",
    "app/models/class_api_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/group.rb", GROUP),
    ("app/models/member.rb", MEMBER),
    ("app/models/class_api_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn class_quoting_reflection_keys_and_empty_lists_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "class-api",
        &FILES,
        &emitted,
        "rh_spec_class_api",
        "ClassApiProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
