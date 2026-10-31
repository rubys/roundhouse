//! PostgreSQL: pluck columns and association reflection, on the Ruby target against native Rails.
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
  database: rh_spec_pluck_columns
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :groups
  has_one :first_group, class_name: "Group"
  has_many :members, through: :groups
end
"#;

const GROUP: &str = r#"class Group < ApplicationRecord
  belongs_to :shop
  has_many :members
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  belongs_to :group
end
"#;

const PROBE: &str = r##"class PluckProbe
  def self.lines
    out = []
    shop = Shop.create!(name: "s")
    Group.create!(shop_id: shop.id, name: "a", number: 1, active: true)
    Group.create!(shop_id: shop.id, name: "b", number: 2, active: false)
    Group.create!(shop_id: shop.id, name: "c", number: nil, active: nil)
    rows = Group.where(shop_id: shop.id).order(:id).pluck(:name, :number, :active)
    out << "three: #{rows.inspect}"
    out << "to_h: #{Group.where(shop_id: shop.id).order(:id).pluck(:name, :number).to_h.inspect}"
    names = []
    Group.where(shop_id: shop.id).order(:id).pluck(:name, :active).each { |name, active| names << "#{name}=#{active.inspect}" }
    out << "destructure: #{names.join(",")}"
    out << "one: #{Group.where(shop_id: shop.id).order(:id).pluck(:name).inspect} #{Group.where(shop_id: shop.id).order(:id).pluck("number").inspect}"
    out << "pick: #{Group.where(shop_id: shop.id).order(:id).pick(:name, :number).inspect} #{Group.where(shop_id: shop.id).order(:id).pick(:name).inspect} #{Group.where(name: "zz").pick(:name, :number).inspect}"
    out << "empty: #{Group.where(name: "zz").pluck(:name, :number).inspect}"
    group = Group.where(shop_id: shop.id).order(:id).first
    belongs = group.association(:shop).reflection
    out << "belongs_to: #{belongs.class.name} #{belongs.name.inspect} #{belongs.macro.inspect} #{belongs.through_reflection?} #{belongs.through_reflection.inspect}"
    many = shop.association(:groups).reflection
    out << "has_many: #{many.class.name} #{many.name.inspect} #{many.through_reflection?} #{many.class == ActiveRecord::Reflection::HasManyReflection}"
    one = Shop.reflect_on_association(:first_group)
    out << "has_one: #{one.class.name} #{one.through_reflection.inspect}"
    through = shop.association(:members).reflection
    out << "through: #{through.through_reflection?} #{through.through_reflection.name.inspect} #{through.through_reflection.class.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    r#"three: [["a", 1, true], ["b", 2, false], ["c", nil, nil]]"#,
    r#"to_h: {"a" => 1, "b" => 2, "c" => nil}"#,
    r#"destructure: a=true,b=false,c=nil"#,
    r#"one: ["a", "b", "c"] [1, 2, nil]"#,
    r#"pick: ["a", 1] "a" nil"#,
    r#"empty: []"#,
    r#"belongs_to: ActiveRecord::Reflection::BelongsToReflection :shop :belongs_to false nil"#,
    r#"has_many: ActiveRecord::Reflection::HasManyReflection :groups false true"#,
    r#"has_one: ActiveRecord::Reflection::HasOneReflection nil"#,
    r#"through: true :groups ActiveRecord::Reflection::HasManyReflection"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/group",
    "app/models/member",
    "app/models/pluck_probe",
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
    ("app/models/pluck_probe.rb", PROBE),
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
fn pluck_columns_and_association_reflection_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "pluck-columns",
        &FILES,
        &emitted,
        "rh_spec_pluck_columns",
        "PluckProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
