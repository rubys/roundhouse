//! PostgreSQL: assignment cast, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE gadgets (
    id bigserial NOT NULL,
    count integer,
    total bigint DEFAULT 0 NOT NULL,
    active boolean,
    ratio double precision,
    price numeric(10,2),
    day date,
    happened_at timestamp(6) without time zone,
    name character varying,
    body text,
    uid uuid,
    legacy character varying,
    owner_id bigint,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);

CREATE TABLE readings (
    id bigserial NOT NULL,
    ratio double precision,
    measure numeric,
    taken_at timestamp(6) without time zone,
    CONSTRAINT readings_pkey PRIMARY KEY (id)
);

CREATE TABLE owners (
    id bigserial NOT NULL,
    CONSTRAINT owners_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_assignment_cast
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const GADGET: &str = r#"class Gadget < ApplicationRecord
  attribute :legacy, :integer
  belongs_to :owner, optional: true
end
"#;

const OWNER: &str = r#"class Owner < ApplicationRecord
end
"#;

const READING: &str = r#"class Reading < ApplicationRecord
end
"#;

const PROBE: &str = r##"class AssignmentCastProbe
  COLUMNS = %i[count total active ratio price day happened_at name body uid legacy]

  def self.show(g)
    COLUMNS.map { |c| "#{c}=#{fmt(g.public_send(c))}" }.join(" ")
  end

  # The instant and the calendar day, not their zone presentation or
  # ActiveSupport's Date#inspect.
  def self.fmt(v)
    case v
    when Time then "Time(#{v.utc.iso8601(6)})"
    when Date then "Date(#{v.iso8601})"
    else v.inspect
    end
  end

  def self.exact(r, ratio, measure)
    "ratio=#{r.ratio} #{r.ratio == ratio} measure=#{r.measure} #{r.measure == measure} taken_at=#{r.taken_at.utc.iso8601(6)} usec=#{r.taken_at.usec}"
  end

  def self.precision
    out = []
    ratio = 0.1 + 0.2
    measure = BigDecimal("12345678901234567.123456789")
    r = Reading.new(ratio: ratio, measure: measure, taken_at: "2026-10-08 12:34:56.123456")
    out << "precision new: #{exact(r, ratio, measure)}"
    r.save!
    out << "precision saved: #{exact(r.reload, ratio, measure)}"
    ratio = 1.0e-7 / 3
    measure = BigDecimal("0.1000000000000000055511151231257827")
    r.update!(ratio: ratio, measure: measure.to_s, taken_at: Time.utc(2026, 10, 8, 12, 34, 56, Rational(9999999, 10)))
    out << "precision update: #{exact(r, ratio, measure)} | #{exact(r.reload, ratio, measure)}"
    r.taken_at = "2026-10-08 12:34:56.1234567"
    r.save!
    out << "precision string: #{r.taken_at.usec} #{r.reload.taken_at.usec}"
    picked = Reading.select(:id, :ratio, :measure, :taken_at).where(id: r.id).first
    out << "precision select: #{picked.ratio == ratio} #{picked.measure == measure} #{picked.taken_at.usec}"
    out
  end

  # A String column keeps its own mutable copy of what is assigned
  # (type/string.rb `::String.new(value)`): mutating it in place neither
  # raises on a frozen string nor reaches the caller's.
  def self.own_copy
    mixed = String.new("Mixed")
    g = Gadget.new(name: mixed, body: "Body".freeze)
    g.name.downcase!
    g.body.upcase!
    g[:legacy] = "7"
    g.name = "Fro".freeze
    g[:body] = "zen".freeze
    "own copy: #{g.name} #{g.body} #{mixed} #{g.name.frozen?} #{g.body.frozen?}"
  end

  def self.lines
    out = precision
    g = Gadget.new(count: "7", total: "12", active: "0", ratio: "1.5", price: "9.99", day: "2026-10-08",
      happened_at: "2026-10-08 12:34:56", name: 7, body: :sym, uid: "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11", legacy: "5")
    out << "new: #{show(g)}"
    g.count = 8.9
    g.total = true
    g.active = "yes"
    g.ratio = 2
    g.price = 3
    g.day = "2026-01-02"
    g.happened_at = "2026-01-02T03:04:05Z"
    g.name = :other
    g.body = 1.5
    g.legacy = 6.7
    out << "write: #{show(g)}"
    g.count = ""
    g.active = ""
    g.ratio = " "
    g.price = ""
    g.day = ""
    g.happened_at = ""
    g.name = ""
    g.legacy = ""
    out << "blank: #{show(g)}"
    g.count = "abc"
    g.active = "off"
    g.ratio = "x"
    g.day = "garbage"
    out << "junk: #{show(g)}"
    g[:count] = "10"
    g[:active] = "t"
    g[:ratio] = "0.25"
    out << "index: #{show(g)}"
    g[:day] = "2026-03-04"
    g[:happened_at] = "2026-03-04 05:06:07"
    out << "index read: #{[:count, :active, :ratio, :price, :day, :happened_at, :uid, :legacy].map { |c| fmt(g[c]) }.join(" ")}"
    g.save!
    out << "saved: #{show(g.reload)}"
    g.update!(count: "11", active: "false", price: "1.239", total: "-3", happened_at: "2026-02-03 04:05:06")
    out << "update: #{show(g)}"
    out << "reload: #{show(g.reload)}"
    out << "new attributes: #{Gadget.new.attributes.inspect}"
    out << "attributes: #{g.attributes.keys.inspect} #{g.attributes.values_at("id", "count", "active", "price", "day", "happened_at", "uid", "legacy").map { |v| fmt(v) }.join(" ")} #{g.attributes["id"] == g.id}"
    g.update!(count: nil, price: nil)
    g[:active] = nil
    g[:ratio] = nil
    g.save!
    out << "nil: #{show(g)} | #{show(g.reload)}"
    o = Owner.create!
    g.update!(owner: o)
    fk = [g.owner_id == o.id]
    g.update!(owner_id: o.id.to_s)
    fk << (g.owner_id == o.id)
    g.update!(owner_id: nil)
    fk << g.reload.owner_id.inspect
    g.owner_id = o.id
    g[:owner_id] = nil
    fk << g.owner_id.inspect
    out << "fk: #{fk.join(" ")}"
    out << own_copy
    out
  end
end
"##;

const EXPECTED: [&str; 19] = [
    "precision new: ratio=0.30000000000000004 true measure=12345678901234567.123456789 true taken_at=2026-10-08T12:34:56.123456Z usec=123456",
    "precision saved: ratio=0.30000000000000004 true measure=12345678901234567.123456789 true taken_at=2026-10-08T12:34:56.123456Z usec=123456",
    "precision update: ratio=3.3333333333333334e-08 true measure=0.1000000000000000055511151231257827 true taken_at=2026-10-08T12:34:56.999999Z usec=999999 | ratio=3.3333333333333334e-08 true measure=0.1000000000000000055511151231257827 true taken_at=2026-10-08T12:34:56.999999Z usec=999999",
    "precision string: 123456 123456",
    "precision select: true true 123456",
    r#"new: count=7 total=12 active=false ratio=1.5 price=0.999e1 day=Date(2026-10-08) happened_at=Time(2026-10-08T12:34:56.000000Z) name="7" body="sym" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=5"#,
    r#"write: count=8 total=1 active=true ratio=2.0 price=0.3e1 day=Date(2026-01-02) happened_at=Time(2026-01-02T03:04:05.000000Z) name="other" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=6"#,
    r#"blank: count=nil total=1 active=nil ratio=nil price=nil day=nil happened_at=nil name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"junk: count=0 total=1 active=false ratio=0.0 price=nil day=nil happened_at=nil name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"index: count=10 total=1 active=true ratio=0.25 price=nil day=nil happened_at=nil name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"index read: 10 true 0.25 nil Date(2026-03-04) Time(2026-03-04T05:06:07.000000Z) "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" nil"#,
    r#"saved: count=10 total=1 active=true ratio=0.25 price=nil day=Date(2026-03-04) happened_at=Time(2026-03-04T05:06:07.000000Z) name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"update: count=11 total=-3 active=false ratio=0.25 price=0.124e1 day=Date(2026-03-04) happened_at=Time(2026-02-03T04:05:06.000000Z) name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"reload: count=11 total=-3 active=false ratio=0.25 price=0.124e1 day=Date(2026-03-04) happened_at=Time(2026-02-03T04:05:06.000000Z) name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"new attributes: {"id" => nil, "count" => nil, "total" => 0, "active" => nil, "ratio" => nil, "price" => nil, "day" => nil, "happened_at" => nil, "name" => nil, "body" => nil, "uid" => nil, "legacy" => nil, "owner_id" => nil}"#,
    r#"attributes: ["id", "count", "total", "active", "ratio", "price", "day", "happened_at", "name", "body", "uid", "legacy", "owner_id"] 1 11 false 0.124e1 Date(2026-03-04) Time(2026-02-03T04:05:06.000000Z) "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" nil true"#,
    r#"nil: count=nil total=-3 active=nil ratio=nil price=nil day=Date(2026-03-04) happened_at=Time(2026-02-03T04:05:06.000000Z) name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil | count=nil total=-3 active=nil ratio=nil price=nil day=Date(2026-03-04) happened_at=Time(2026-02-03T04:05:06.000000Z) name="" body="1.5" uid="a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11" legacy=nil"#,
    r#"fk: true true nil nil"#,
    r#"own copy: Fro zen Mixed false false"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/owner",
    "app/models/reading",
    "app/models/gadget",
    "app/models/assignment_cast_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/gadget.rb", GADGET),
    ("app/models/owner.rb", OWNER),
    ("app/models/reading.rb", READING),
    ("app/models/assignment_cast_probe.rb", PROBE),
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
fn assignment_cast_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "assignment-cast",
        &FILES,
        &emitted,
        "rh_spec_assignment_cast",
        "AssignmentCastProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
