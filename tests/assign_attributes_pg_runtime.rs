//! PostgreSQL: assign attributes, on the Ruby target against native Rails.
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
    name character varying,
    code character varying DEFAULT 'std'::character varying,
    qty integer DEFAULT '3' NOT NULL,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_assign_attributes
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// Writers that log what they are given and what the record holds when
/// they run: the log shows which writers run and in what order.
const WIDGET: &str = r##"class Widget < ApplicationRecord
  attr_accessor :note
  after_initialize do
    record_init
  end

  def name=(value)
    log << "name=#{value.inspect} code=#{code.inspect}"
    super
  end

  def code=(value)
    log << "code=#{value.inspect} name=#{name.inspect}"
    super
  end

  def note=(value)
    log << "note=#{value.inspect}"
    @note = value
  end

  def log
    @log ||= []
  end

  private

  def record_init
    log << "after_initialize #{new_record?} #{name.inspect} #{code.inspect} #{qty.inspect}"
  end
end
"##;

const PROBE: &str = r##"class AssignAttributesProbe
  def self.lines
    out = []
    w = Widget.new
    out << "defaults: #{w.log.inspect} #{w.code.inspect} #{w.qty.inspect} #{w.name.inspect}"
    w = Widget.new(code: "b", name: "a", note: "n", qty: "4")
    out << "order: #{w.log.inspect} #{w.qty.inspect} #{w.note.inspect}"
    w = Widget.new(name: "a", code: "b")
    out << "reversed: #{w.log.inspect}"
    w = Widget.new("note" => "s", "name" => "c")
    out << "strings: #{w.log.inspect}"
    begin
      Widget.new(name: "x", bogus: 1, code: "y")
    rescue ActiveModel::UnknownAttributeError => e
      out << "unknown new: #{e.class} #{e.message} #{e.attribute} #{e.record.log.inspect} #{e.record.code.inspect}"
    end
    c = Widget.create!(name: "d", code: "e")
    out << "create: #{c.log.inspect} #{c.persisted?}"
    f = Widget.find(c.id)
    out << "load: #{f.log.inspect} #{f.name} #{f.code} #{f.qty}"
    f.log.clear
    out << "update: #{f.update(qty: 5, note: "u", code: "z")} #{f.log.inspect} #{Widget.find(c.id).qty}"
    begin
      f.update(nope: 1)
    rescue ActiveRecord::UnknownAttributeError => e
      out << "unknown update: #{e.message} #{Widget.find(c.id).code}"
    end
    f.log.clear
    f.update!(name: "g")
    out << "update!: #{f.log.inspect} #{Widget.find(c.id).name}"
    out << "where: #{Widget.where(code: "z").map(&:log).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    r#"defaults: ["after_initialize true nil \"std\" 3"] "std" 3 nil"#,
    r#"order: ["code=\"b\" name=nil", "name=\"a\" code=\"b\"", "note=\"n\"", "after_initialize true \"a\" \"b\" 4"] 4 "n""#,
    r#"reversed: ["name=\"a\" code=\"std\"", "code=\"b\" name=\"a\"", "after_initialize true \"a\" \"b\" 3"]"#,
    r#"strings: ["note=\"s\"", "name=\"c\" code=\"std\"", "after_initialize true \"c\" \"std\" 3"]"#,
    r#"unknown new: ActiveModel::UnknownAttributeError unknown attribute 'bogus' for Widget. bogus ["name=\"x\" code=\"std\""] "std""#,
    r#"create: ["name=\"d\" code=\"std\"", "code=\"e\" name=\"d\"", "after_initialize true \"d\" \"e\" 3"] true"#,
    r#"load: ["after_initialize false \"d\" \"e\" 3"] d e 3"#,
    r#"update: true ["note=\"u\"", "code=\"z\" name=\"d\""] 5"#,
    r#"unknown update: unknown attribute 'nope' for Widget. z"#,
    r#"update!: ["name=\"g\" code=\"z\""] g"#,
    r#"where: [["after_initialize false \"g\" \"z\" 5"]]"#,
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/assign_attributes_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn new_create_and_update_assign_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "assign-attributes",
        &FILES,
        &emitted,
        "rh_spec_assign_attributes",
        "AssignAttributesProbe",
        &["app/models/application_record", "app/models/widget", "app/models/assign_attributes_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The compiler side: `initialize` writes the defaults through the
/// generated module's writers and then assigns the given attributes;
/// `update` assigns them before saving.
#[test]
#[ignore = "fork spec: postgresql"]
fn initialize_and_update_call_assign_attributes() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).expect("widget.rb");
    for expected in [
        "    self._generated_code = \"std\"\n",
        "    assign_attributes(attrs) if !(attrs.equal?(ActiveRecord::Base::HYDRATE_ATTRS))\n",
        "  def update(attrs)\n    assign_attributes(attrs)\n    self.save\n  end\n",
        "    alias_method :_generated_code=, :code=\n",
    ] {
        assert!(widget.contains(expected), "missing {expected:?} in:\n{widget}");
    }
    assert!(!widget.contains("attrs[:"), "{widget}");
}
