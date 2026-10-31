//! PostgreSQL: after initialize symbol, on the Ruby target against native Rails.
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
    status character varying,
    name character varying,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);

CREATE TABLE animals (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT animals_pkey PRIMARY KEY (id)
);

CREATE TABLE vehicles (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT vehicles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_after_initialize_symbol
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  after_initialize :set_defaults, if: :new_record?
  after_initialize :mark_loaded, unless: :new_record?
  after_initialize :first_mark, :second_mark
  after_initialize :prep, prepend: true

  def trail
    @trail ||= []
  end

  def set_defaults
    self.status ||= "draft"
    trail << "defaults"
  end

  def mark_loaded
    trail << "loaded"
  end

  def first_mark
    trail << "first"
  end

  def second_mark
    trail << "second"
  end

  def prep
    trail << "prep"
  end
end
"#;

const ANIMAL: &str = r#"class Animal < ApplicationRecord
  after_initialize :base_init

  def trail
    @trail ||= []
  end

  def base_init
    trail << "animal"
  end
end
"#;

const DOG: &str = r#"class Dog < Animal
  after_initialize :dog_init, if: :new_record?

  def dog_init
    trail << "dog"
  end
end
"#;

const CAT: &str = r#"class Cat < Animal
  after_initialize do
    trail << "cat"
  end
end
"#;

const VEHICLE: &str = r#"class Vehicle < ApplicationRecord
  self.inheritance_column = nil

  def trail
    @trail ||= []
  end
end
"#;

const CAR: &str = r#"class Car < Vehicle
  after_initialize :car_init

  def car_init
    trail << "car"
  end
end
"#;

const PROBE: &str = r##"class AfterInitializeSymbolProbe
  def self.lines
    out = []
    w = Widget.new
    out << "widget new: #{w.trail.join(",")} #{w.status}"
    Widget.create!(name: "a", status: "live")
    out << "widget loaded: #{Widget.first.trail.join(",")} #{Widget.first.status}"
    out << "widget kept: #{Widget.new(status: "set").status}"
    out << "dog new: #{Dog.new.trail.join(",")}"
    Dog.create!(name: "rex")
    Cat.create!(name: "tom")
    out << "dog loaded: #{Dog.first.trail.join(",")} #{Animal.order(:id).map { |a| a.trail.join("+") }.join(",")}"
    out << "cat new: #{Cat.new.trail.join(",")} #{Animal.new.trail.join(",")}"
    out << "car new: #{Car.new.trail.join(",")} #{Vehicle.new.trail.size}"
    Car.create!(name: "c")
    out << "car loaded: #{Car.first.trail.join(",")} #{Vehicle.first.trail.size}"
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "widget new: defaults,first,second,prep draft",
    "widget loaded: loaded,first,second,prep live",
    "widget kept: set",
    "dog new: animal,dog",
    "dog loaded: animal animal,animal+cat",
    "cat new: animal,cat animal",
    "car new: car 0",
    "car loaded: car 0",
];

/// What native Rails requires, in order.
const LOADS: [&str; 8] = [
    "app/models/application_record",
    "app/models/widget",
    "app/models/animal",
    "app/models/dog",
    "app/models/cat",
    "app/models/vehicle",
    "app/models/car",
    "app/models/after_initialize_symbol_probe",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/animal.rb", ANIMAL),
    ("app/models/dog.rb", DOG),
    ("app/models/cat.rb", CAT),
    ("app/models/vehicle.rb", VEHICLE),
    ("app/models/car.rb", CAR),
    ("app/models/after_initialize_symbol_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The symbol forms fold into the model's `after_initialize` override,
/// in declaration order, `prepend: true` included.
#[test]
#[ignore = "fork spec: postgresql"]
fn symbol_after_initialize_folds_into_the_hook() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    let hook = widget
        .split("def after_initialize")
        .nth(1)
        .unwrap_or_else(|| panic!("no after_initialize:\n{widget}"));
    let at = |s: &str| hook.find(s).unwrap_or_else(|| panic!("missing {s}:\n{widget}"));
    assert!(at("set_defaults") < at("mark_loaded") && at("mark_loaded") < at("first_mark"), "{widget}");
    assert!(at("first_mark") < at("second_mark") && at("second_mark") < at("prep"), "{widget}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn symbol_after_initialize_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "after-initialize-symbol",
        &FILES,
        &emitted,
        "rh_spec_after_initialize_symbol",
        "AfterInitializeSymbolProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
