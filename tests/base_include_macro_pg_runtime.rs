//! PostgreSQL: base include macro, on the Ruby target against native Rails.
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
    code character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_base_include_macro
"#;

const LOCK: &str = "PATH\n  remote: vendor/gems/tagging-1.0.0\n  specs:\n    tagging (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  tagging!\n";

const GEM_MAIN: &str = r#"require_relative "tagging/hooks"
require_relative "tagging/upcase_type"
require_relative "tagging/railtie"
"#;

/// MoneyColumn::Railtie's shape.
const GEM_RAILTIE: &str = r#"require "rails/railtie"

module Tagging
  class Railtie < Rails::Railtie
    ActiveSupport.on_load(:active_record) do
      ActiveRecord::Base.send(:include, Tagging::Hooks)
    end
  end
end
"#;

/// MoneyColumn::ActiveRecordHooks' shape: the included hook extends
/// ClassMethods, whose macro retypes its columns and defines readers.
const GEM_HOOKS: &str = r##"module Tagging
  module Hooks
    def self.included(base)
      base.extend(ClassMethods)
    end

    def tag_for(column)
      "#{self.class.tag_prefix}:#{public_send(column)}"
    end

    module ClassMethods
      attr_reader :tag_prefix

      def tagged(*columns, prefix: nil)
        @tag_prefix = prefix || name.downcase
        columns.flatten.each do |column|
          column_string = column.to_s.freeze
          attribute(column_string, Tagging::UpcaseType.new)
          define_method("#{column}_tag") do
            tag_for(column_string)
          end
        end
      end
    end
  end
end
"##;

const GEM_TYPE: &str = r#"module Tagging
  class UpcaseType < ActiveModel::Type::String
    def cast(value)
      value.nil? ? nil : value.to_s.upcase
    end
  end
end
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  tagged :code, prefix: "shop"
end
"#;

const PROBE: &str = r##"class BaseIncludeProbe
  def self.lines
    out = []
    s = Shop.create!(name: "abc", code: "xy")
    out << "cast: #{s.code.inspect} #{Shop.find(s.id).code.inspect}"
    out << "reader: #{s.code_tag.inspect}"
    out << "include: #{ActiveRecord::Base.include?(Tagging::Hooks)} #{Shop.tag_prefix.inspect}"
    out
  end
end
"##;

/// Native boot: Rails::Railtie from the pinned railties, then the gem as
/// Bundler.require loads it. Native only.
const NATIVE_BOOT: &str = r#"railties = $LOAD_PATH.grep(%r{/activerecord/lib\z}).first.sub(%r{/activerecord/lib\z}, "/railties/lib")
$LOAD_PATH.unshift(railties)
require "rails/railtie"
require_relative "vendor/gems/tagging-1.0.0/lib/tagging"
"#;

const EXPECTED: [&str; 3] = [r#"cast: "XY" "XY""#, r#"reader: "shop:XY""#, r#"include: true "shop""#];

const FILES: [(&str, &str); 15] = [
    ("roundhouse.yml", ""),
    ("Gemfile", "source \"https://rubygems.org\"\n\ngem \"tagging\"\n"),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("vendor/gems/tagging-1.0.0/lib/tagging.rb", GEM_MAIN),
    ("vendor/gems/tagging-1.0.0/lib/tagging/railtie.rb", GEM_RAILTIE),
    ("vendor/gems/tagging-1.0.0/lib/tagging/hooks.rb", GEM_HOOKS),
    ("vendor/gems/tagging-1.0.0/lib/tagging/upcase_type.rb", GEM_TYPE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/base_include_probe.rb", PROBE),
    ("native_boot.rb", NATIVE_BOOT),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn a_railtie_base_include_macro_matches_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "base-include-macro",
        &FILES,
        &emitted,
        "rh_spec_base_include_macro",
        "BaseIncludeProbe",
        &["native_boot", "app/models/application_record", "app/models/shop", "app/models/base_include_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The macro call is replayed in the class body.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_base_include_macro_is_replayed() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).expect("shop.rb");
    assert!(shop.contains("tagged(:code, prefix: \"shop\")") || shop.contains("tagged :code, prefix: \"shop\""), "{shop}");
}

/// An override of `_create_record`: the runtime base never calls it, so
/// a macro of the include is refused, naming the override.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_hooks_module_overriding_unported_framework_methods_refuses_the_macro() {
    let hooks = GEM_HOOKS.replace(
        "    def tag_for(column)",
        "    private\n\n    def _create_record(*)\n      @tag_cache = {}\n      super\n    end\n\n    public\n\n    def tag_for(column)",
    );
    let (_emitted, errors) =
        overlay().write("vendor/gems/tagging-1.0.0/lib/tagging/hooks.rb", &hooks).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("tagged") && e.contains("model macro call on `Shop` not lowered") && e.contains("`_create_record`")),
        "{}",
        errors.join("\n")
    );
}
