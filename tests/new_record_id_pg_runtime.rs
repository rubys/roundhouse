//! PostgreSQL: new record id, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE owners (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT owners_pkey PRIMARY KEY (id)
);

CREATE TABLE pets (
    id bigserial NOT NULL,
    owner_id bigint,
    name character varying,
    CONSTRAINT pets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_new_record_id
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const OWNER: &str = r#"class Owner < ApplicationRecord
  has_many :pets
end
"#;

const PET: &str = r#"class Pet < ApplicationRecord
  belongs_to :owner, optional: true
end
"#;

const PROBE: &str = r##"class NewRecordIdProbe
  def self.lines
    out = []
    o = Owner.new(name: "o")
    out << "new: #{o.id.inspect} #{o.attributes["id"].inspect} #{o.to_param.inspect} #{o.new_record?}"
    out << "given: #{Owner.new(id: 5).id.inspect} #{Owner.new(id: "6").id.inspect} #{Owner.new(id: nil).id.inspect}"
    a = Owner.new
    b = Owner.new
    out << "equality: #{a == b} #{a == a} #{[a, b].uniq.size} #{a.eql?(b)}"
    p = Pet.new(name: "p")
    p.owner = o
    out << "fk from unsaved: #{p.owner_id.inspect} #{p.owner.equal?(o)}"
    o.save!
    out << "saved: #{o.id.is_a?(Integer)} #{o.id > 0} #{o.to_param == o.id.to_s}"
    p.save!
    out << "autosaved fk: #{p.reload.owner_id == o.id}"
    o3 = Owner.create!(name: "o3")
    q = Pet.new(name: "q")
    q.owner = o
    q.owner_id = o3.id
    q.save!
    out << "stale target: #{q.reload.owner_id == o3.id}"
    o2 = Owner.create!(name: "o2")
    id = o2.id
    o2.destroy
    out << "destroyed: #{o2.id == id} #{o2.destroyed?}"
    out << "pets of new: #{Owner.new.pets.to_a.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    r#"new: nil nil nil true"#,
    r#"given: 5 6 nil"#,
    r#"equality: false true 2 false"#,
    r#"fk from unsaved: nil true"#,
    r#"saved: true true true"#,
    r#"autosaved fk: true"#,
    r#"stale target: true"#,
    r#"destroyed: true true"#,
    r#"pets of new: []"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/owner",
    "app/models/pet",
    "app/models/new_record_id_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/owner.rb", OWNER),
    ("app/models/pet.rb", PET),
    ("app/models/new_record_id_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn new_record_id_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "new-record-id",
        &FILES,
        &emitted,
        "rh_spec_new_record_id",
        "NewRecordIdProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
