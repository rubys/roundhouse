//! PostgreSQL: program concern include, on the Ruby target against native Rails.
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
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_program_concern_include
"#;

const LOCK: &str = "PATH\n  remote: vendor/gems/digesting-1.0.0\n  specs:\n    digesting (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  digesting!\n";

/// lib/paquito.rb's shape.
const GEM_MAIN: &str = r#"require "digesting/types"

module Digesting
  def self.digest_of(klass)
    klass.column_names.join(",")
  end
end

if defined? ActiveSupport
  ActiveSupport.on_load(:active_record) do
    require "digesting/columns_digest_cache"

    include Digesting::ColumnsDigestCache
  end
end
"#;

/// Paquito::ColumnsDigestCache's shape.
const GEM_CACHE: &str = r#"module Digesting
  module ColumnsDigestCache
    extend ActiveSupport::Concern

    class_methods do
      def digesting_columns_digest
        @digesting_columns_digest ||= Digesting.digest_of(self)
      end
    end
  end
end
"#;

/// Paquito::Types and Types::ActiveRecordPacker's shape.
const GEM_TYPES: &str = r#"module Digesting
  module Types
    def self.register(registry, types)
      types.each { |type| registry << type.name }
    end

    class Packer
      registry = []
      Types.register(registry, [Symbol, Time])
      REGISTRY = registry.freeze
    end
  end
end
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = "class Shop < ApplicationRecord\nend\n";

const PROBE: &str = r##"class ProgramConcernProbe
  def self.lines
    out = []
    out << "digest: #{Shop.digesting_columns_digest.inspect}"
    out << "include: #{ActiveRecord::Base.include?(Digesting::ColumnsDigestCache)} #{ApplicationRecord.respond_to?(:digesting_columns_digest)}"
    out << "registry: #{Digesting::Types::Packer::REGISTRY.inspect}"
    out
  end
end
"##;

/// Native boot: the gem as Bundler.require loads it. Native only.
const NATIVE_BOOT: &str = r#"$LOAD_PATH.unshift(File.expand_path("vendor/gems/digesting-1.0.0/lib", __dir__))
require "digesting"
"#;

const EXPECTED: [&str; 3] = [r#"digest: "id,name""#, "include: true true", r#"registry: ["Symbol", "Time"]"#];

const FILES: [(&str, &str); 14] = [
    ("roundhouse.yml", ""),
    ("Gemfile", "source \"https://rubygems.org\"\n\ngem \"digesting\"\n"),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("vendor/gems/digesting-1.0.0/lib/digesting.rb", GEM_MAIN),
    ("vendor/gems/digesting-1.0.0/lib/digesting/columns_digest_cache.rb", GEM_CACHE),
    ("vendor/gems/digesting-1.0.0/lib/digesting/types.rb", GEM_TYPES),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/program_concern_probe.rb", PROBE),
    ("native_boot.rb", NATIVE_BOOT),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_file_program_concern_include_matches_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "program-concern-include",
        &FILES,
        &emitted,
        "rh_spec_program_concern_include",
        "ProgramConcernProbe",
        &["native_boot", "app/models/application_record", "app/models/shop", "app/models/program_concern_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The program statement's include is followed by the Concern's
/// `ClassMethods` extend, as `append_features` performs it.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_file_program_concern_include_extends_class_methods() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let program = walk(&emitted)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rb"))
        .filter_map(|p| std::fs::read_to_string(&p).ok())
        .find(|text| text.contains("include Digesting::ColumnsDigestCache"))
        .expect("the program statement's include");
    let at = program.find("include Digesting::ColumnsDigestCache").unwrap();
    assert!(
        program[at..].lines().nth(1).is_some_and(|l| l.trim() == "extend ::Digesting::ColumnsDigestCache::ClassMethods"),
        "{program}"
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// The packer body's `Types.register(registry, ...)` is written in place.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_class_body_call_of_an_app_class_method_is_kept() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert!(!errors.iter().any(|e| e.contains("Types.register")), "{}", errors.join("\n"));
    let packer = walk(&emitted)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rb"))
        .filter_map(|p| std::fs::read_to_string(&p).ok())
        .find(|text| text.contains("class Packer"))
        .expect("the packer");
    assert!(packer.contains("Types.register("), "{packer}");
}
