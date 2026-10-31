//! PostgreSQL: sti base class method implicit self, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE regions (
    id bigserial NOT NULL,
    type character varying NOT NULL,
    code character varying,
    CONSTRAINT regions_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_base_implicit_self
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// An STI base whose class methods use the implicit self.
const REGION: &str = r#"module Markets
  class Region < ApplicationRecord
    validates :type, presence: true

    scope :ca_or_on, -> { where(code: "CA").or(where(code: "ON")) }
    scope :not_ca_or_nil, -> { where.not(code: "CA").or(where(code: nil)) }

    class << self
      def find_or_initialize_all(attribute_sets)
        return [] if attribute_sets.empty?

        attribute_sets.map! { |attributes| new(attributes).attributes.slice(*attributes.keys) }
        existing = attribute_sets
          .reduce(none) { |scope, attributes| scope.or(where(attributes)) }
          .index_by { |record| record.attributes.slice("code") }
        attribute_sets.map { existing[_1] || new(_1) }
      end

      def make(code)
        create!(code: code)
      end

      def built(code)
        new(code: code)
      end
    end

    def self.coded(code)
      where(code: code).to_a
    end

    def self.nothing
      none.to_a
    end

    def self.fresh(code)
      new(code: code)
    end
  end
end
"#;

const COUNTRY: &str = r#"module Markets
  class Country < Region
  end
end
"#;

const SUBDIVISION: &str = r#"module Markets
  class Subdivision < Region
  end
end
"#;

const PROBE: &str = r##"class StiBaseImplicitSelfProbe
  def self.lines
    out = []
    Markets::Country.create!(code: "CA")
    Markets::Subdivision.make("ON")
    found = Markets::Subdivision.find_or_initialize_all([{ "code" => "ON" }, { "code" => "QC" }])
    out << "found: #{found.map { |r| "#{r.class.name}:#{r.code}:#{r.persisted?}" }.join(",")}"
    found.each(&:save!)
    out << "saved: #{Markets::Region.order(:code).map { |r| "#{r.class.name}:#{r.code}" }.join(",")}"
    out << "none: #{Markets::Country.find_or_initialize_all([{ "code" => "ON" }]).map { |r| "#{r.class.name}:#{r.persisted?}" }.join(",")}"
    out << "built: #{Markets::Subdivision.built("X").class.name} #{Markets::Country.fresh("Y").class.name} #{Markets::Region.fresh("Z").class.name}"
    out << "where: #{Markets::Country.coded("ON").size} #{Markets::Subdivision.coded("ON").map { |r| r.class.name }.join(",")} #{Markets::Region.coded("CA").map { |r| r.class.name }.join(",")} #{Markets::Country.nothing.size}"
    out << "or scopes: #{Markets::Region.ca_or_on.map(&:code).sort.join(",")} #{Markets::Subdivision.ca_or_on.map(&:code).sort.join(",")} #{Markets::Region.not_ca_or_nil.map(&:code).sort.join(",")}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "found: Markets::Subdivision:ON:true,Markets::Subdivision:QC:false",
    "saved: Markets::Country:CA,Markets::Subdivision:ON,Markets::Subdivision:QC",
    "none: Markets::Country:false",
    "built: Markets::Subdivision Markets::Country Markets::Region",
    "where: 0 Markets::Subdivision Markets::Country 0",
    "or scopes: CA,ON ON ON,QC",
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/markets/region",
    "app/models/markets/country",
    "app/models/markets/subdivision",
    "app/models/sti_base_implicit_self_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\nend\n",
    ),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/markets/region.rb", REGION),
    ("app/models/markets/country.rb", COUNTRY),
    ("app/models/markets/subdivision.rb", SUBDIVISION),
    ("app/models/sti_base_implicit_self_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| {
            app.write(path, text)
        })
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The base's implicit-self `new` / `where` / `none` / `create!` stay on
/// `self` (or the relation it threads) rather than naming the base
/// constant.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_base_class_methods_keep_implicit_self() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let region = std::fs::read_to_string(emitted.join("app/models/markets/region.rb")).unwrap();
    for wrong in [
        "Markets::Region.new(",
        "ActiveRecord::Relation.new(Markets::Region)",
        "Markets::Region.create!(",
        "Markets::Region.none",
        "Markets::Region.where(",
    ] {
        assert!(!region.contains(wrong), "found {wrong}:\n{region}");
    }
    // `none` and the block's `where` each read their own copy of the
    // threaded relation; the runtime's chain methods mutate in place.
    assert!(
        region.contains("reduce(__rel.spawn.none) { |scope, attributes| scope.or(__rel.spawn.where(attributes)) }"),
        "{region}"
    );
    assert!(region.contains("new(attributes)") && region.contains("create!(code: code)"), "{region}");
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_base_class_methods_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-base-implicit-self",
        &FILES,
        &emitted,
        "rh_spec_sti_base_implicit_self",
        "StiBaseImplicitSelfProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
