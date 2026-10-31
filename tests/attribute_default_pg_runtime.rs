//! PostgreSQL: attribute default, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    state character varying DEFAULT 'old'::character varying,
    count integer,
    flag boolean,
    token character varying,
    kind character varying,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);

CREATE TABLE requests (
    id uuid NOT NULL,
    email character varying,
    CONSTRAINT requests_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_attribute_default
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  FRESH = "fresh"
  TOKENS = []

  attribute :state, :string, default: FRESH
  attribute :count, :integer, default: "7"
  attribute :flag, :boolean, default: "1"
  attribute :token, default: -> { TOKENS << 1; "t-" + name.to_s }
end
"#;

const SPECIAL_WIDGET: &str = r#"class SpecialWidget < Widget
  attribute :kind, :string, default: "special"
end
"#;

const REQUEST: &str = r#"class Request < ApplicationRecord
  MINTED = []

  self.primary_key = :id

  attribute :id, :uid, default: -> { MINTED << 1; format("0190a3b2-0000-7000-8000-%012d", MINTED.size) }
end
"#;

const UID_TYPE: &str = r##"module Probe
  class Uid
    attr_reader :hex

    def initialize(hex)
      @hex = hex.to_s.downcase
    end

    def to_s
      hex
    end

    def ==(other)
      other.is_a?(Uid) && other.hex == hex
    end

    def inspect
      "#<Uid #{hex}>"
    end
  end

  class UidType < ActiveModel::Type::Value
    def type
      :uid
    end

    def cast(value)
      return nil if value.nil? || value == ""
      value.is_a?(Uid) ? value : Uid.new(value)
    end

    def serialize(value)
      cast(value)&.to_s
    end

    def deserialize(value)
      value.nil? ? nil : Uid.new(value)
    end
  end
end
"##;

const INITIALIZER: &str = "ActiveRecord::Type.register(:uid, Probe::UidType, adapter: :postgresql)\n";

const PROBE: &str = r##"class AttributeDefaultProbe
  def self.lines
    out = []
    w = Widget.new
    out << "new: #{w.state.inspect} #{w.count.inspect} #{w.flag.inspect} #{w.token.inspect} #{w.kind.inspect}"
    given = Widget.new(state: "given", count: 2, flag: false, token: nil)
    out << "given: #{given.state.inspect} #{given.count.inspect} #{given.flag.inspect} #{given.token.inspect}"
    s = SpecialWidget.new
    out << "sti: #{s.kind.inspect} #{s.state.inspect} #{Widget.new.kind.inspect}"
    a = Widget.create!(name: "a")
    b = Widget.create!(name: "b", state: nil, count: nil)
    c = SpecialWidget.create!(name: "c")
    out << "raw: #{Widget.order(:id).map { |r| [r.name, r.state, r.count, r.flag, r.token, r.kind] }.inspect}"
    out << "loaded: #{Widget.find(b.id).state.inspect} #{Widget.find(b.id).count.inspect} #{Widget.find(c.id).class} #{Widget.find(c.id).kind.inspect}"
    out << "where: #{Widget.where(state: "fresh").count} #{Widget.where(count: 7).count} #{a.reload.flag.inspect}"
    out << "types: #{Widget.type_for_attribute(:count).type.inspect} #{Widget.type_for_attribute(:token).type.inspect}"
    req = Request.new(email: "a")
    out << "pk new: #{req.id.class} #{req.id.to_s.length} #{Request.new(id: "0190A3B2-0000-7000-8000-0000000000FF").id.inspect}"
    req.save!
    found = Request.find(req.id)
    out << "pk find: #{found.id.class} #{found.id == req.id} #{Request.where(id: req.id).count} #{Request.exists?(req.id)}"
    given = Request.create!(id: "0190A3B2-0000-7000-8000-0000000000AA", email: "b")
    out << "pk given: #{given.reload.id.inspect} #{Request.find("0190a3b2-0000-7000-8000-0000000000aa").email}"
    found.email = "c"
    found.save!
    out << "pk update: #{Request.find(req.id).email} #{Request.count} #{Request.type_for_attribute(:id).type.inspect}"
    tokens = Widget::TOKENS.size
    minted = Request::MINTED.size
    Widget.find(a.id)
    Widget.find(c.id)
    Widget.where(name: "b").to_a
    Widget.all.load
    Widget.order(:id).first
    SpecialWidget.all.to_a
    a.reload
    Request.find(req.id)
    Request.where(email: "c").to_a
    Request.all.load
    found.reload
    out << "proc calls on load: #{Widget::TOKENS.size - tokens} #{Request::MINTED.size - minted}"
    Widget.new(token: "x")
    Request.new(id: "0190A3B2-0000-7000-8000-0000000000BB")
    out << "proc calls on new given: #{Widget::TOKENS.size - tokens} #{Request::MINTED.size - minted}"
    Widget.new.token
    Request.new.id
    out << "proc calls on new: #{Widget::TOKENS.size - tokens} #{Request::MINTED.size - minted}"
    out
  end
end
"##;

const EXPECTED: [&str; 14] = [
    r#"new: "fresh" 7 true "t-Widget" nil"#,
    r#"given: "given" 2 false nil"#,
    r#"sti: "special" "fresh" nil"#,
    r#"raw: [["a", "fresh", 7, true, "t-Widget", nil], ["b", nil, nil, true, "t-Widget", nil], ["c", "fresh", 7, true, "t-Widget", "special"]]"#,
    r#"loaded: nil nil SpecialWidget "special""#,
    "where: 2 2 true",
    "types: :integer :string",
    "pk new: Probe::Uid 36 #<Uid 0190a3b2-0000-7000-8000-0000000000ff>",
    "pk find: Probe::Uid true 1 true",
    "pk given: #<Uid 0190a3b2-0000-7000-8000-0000000000aa> b",
    "pk update: c 2 :uid",
    "proc calls on load: 0 0",
    "proc calls on new given: 0 0",
    "proc calls on new: 1 1",
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "lib/probe/uid_type",
    "config/initializers/uid_types",
    "app/models/application_record",
    "app/models/widget",
    "app/models/special_widget",
    "app/models/request",
    "app/models/attribute_default_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/special_widget.rb", SPECIAL_WIDGET),
    ("app/models/attribute_default_probe.rb", PROBE),
    ("app/models/request.rb", REQUEST),
    ("lib/probe/uid_type.rb", UID_TYPE),
    ("config/initializers/uid_types.rb", INITIALIZER),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries. The `attribute` calls are claimed: they
/// run as written at class load.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the model's `initialize` asks for the declared
/// default of each named column, and the calls are carried.
#[test]
#[ignore = "fork spec: postgresql"]
fn attribute_default_runs_at_class_load() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    for expected in [
        r#"self.class._attribute_default("state")"#,
        r#"self.class._attribute_default("kind")"#,
        r#"["state", "count", "flag", "token", "kind"]"#,
        "  attribute :state, :string, default: FRESH\n",
    ] {
        assert!(widget.contains(expected), "missing {expected:?} in:\n{widget}");
    }
    let request = std::fs::read_to_string(emitted.join("app/models/request.rb")).unwrap();
    assert!(request.contains(r#"self.class._attribute_default("id")"#), "request:\n{request}");
    let special = std::fs::read_to_string(emitted.join("app/models/special_widget.rb")).unwrap();
    assert!(special.contains(r#"  attribute :kind, :string, default: "special""#), "special_widget:\n{special}");
}

/// A `default:` call with another option stays refused.
#[test]
#[ignore = "fork spec: postgresql"]
fn attribute_default_with_other_options_is_refused() {
    let app = overlay().write(
        "app/models/widget.rb",
        "class Widget < ApplicationRecord\n  attribute :state, :string, default: \"x\", limit: 5\nend\n",
    );
    let (_emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        real_errors(&errors).iter().any(|e| e.contains("attribute") && e.contains("Widget")),
        "{}",
        errors.join("\n")
    );
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn attribute_default_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "attribute-default",
        &FILES,
        &emitted,
        "rh_spec_attribute_default",
        "AttributeDefaultProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
