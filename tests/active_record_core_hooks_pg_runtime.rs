//! PostgreSQL: active record core hooks, on the Ruby target against native Rails.
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
  database: rh_spec_active_record_core_hooks
"#;

const LOCK: &str = "PATH\n  remote: vendor/gems/tracing-1.0.0\n  specs:\n    tracing (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  tracing!\n";

const GEM_MAIN: &str = r#"require_relative "tracing/hooks"
require_relative "tracing/railtie"
"#;

/// MoneyColumn::Railtie's shape.
const GEM_RAILTIE: &str = r#"require "rails/railtie"

module Tracing
  class Railtie < Rails::Railtie
    ActiveSupport.on_load(:active_record) do
      ActiveRecord::Base.send(:include, Tracing::Hooks)
    end
  end
end
"#;

/// MoneyColumn::ActiveRecordHooks' instance half: a per-record cache set
/// up in `init_internals`, cleared on `reload` and `dup`, and the raw
/// attributes of the assignment in progress.
const GEM_HOOKS: &str = r##"module Tracing
  module Hooks
    def reload(*)
      trace << "reload"
      @trace_cache = {}
      super
    end

    def initialize_dup(*)
      @trace_cache = {}
      @trace = trace + ["dup"]
      super
    end

    def trace
      @trace ||= []
    end

    def trace_cache_ready?
      !@trace_cache.nil?
    end

    def trace_raw_keys
      @trace_raw && @trace_raw.keys
    end

    def remember(column)
      @trace_cache[column] = public_send(column)
    end

    def cached(column)
      @trace_cache[column]
    end

    def cache_size
      @trace_cache.size
    end

    private

    def init_internals
      @trace_cache = {}
      trace << "init_internals"
      super
    end

    def _assign_attributes(new_attributes)
      @trace_raw = new_attributes.symbolize_keys
      trace << "assign:#{@trace_raw.keys.join(",")}"
      super
    ensure
      @trace_raw = nil
    end
  end
end
"##;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// A writer that is not a column: what it sees shows the order the
/// assignment sends the keys in and the raw attributes it runs under.
const SHOP: &str = r##"class Shop < ApplicationRecord
  attr_reader :note

  def note=(value)
    @note = "#{value} name=#{name.inspect} raw=#{trace_raw_keys.inspect}"
  end
end
"##;

const PROBE: &str = r##"class CoreHooksProbe
  def self.lines
    out = []
    s = Shop.new
    out << "new: #{s.trace.inspect} cache=#{s.trace_cache_ready?}"
    s.name = "a"
    s.code = "x"
    s.save!
    t = Shop.find(s.id)
    out << "load: #{t.trace.inspect} cache=#{t.trace_cache_ready?}"
    r = t.assign_attributes(note: "n1", name: "b")
    out << "assign: #{r.inspect} #{t.note} | #{t.name} #{t.trace.inspect} raw=#{t.trace_raw_keys.inspect}"
    t.assign_attributes("name" => "c", "note" => "n2")
    out << "strings: #{t.note} | #{t.name}"
    t.attributes = { code: "y" }
    out << "attributes=: #{t.code} #{t.trace.last}"
    out << "empty: #{t.assign_attributes({}).inspect} #{t.trace.size}"
    begin
      t.assign_attributes(bogus: 1)
    rescue ActiveRecord::UnknownAttributeError => e
      out << "unknown: #{e.class} #{e.message} #{e.attribute} #{e.record.equal?(t)} #{e.is_a?(NoMethodError)}"
    end
    begin
      t.assign_attributes("x")
    rescue ArgumentError => e
      out << "not a hash: #{e.message}"
    end
    t.save!
    n = Shop.new(note: "n3", code: "w")
    out << "new attrs: #{n.note} | #{n.code} #{n.trace.inspect}"
    c = Shop.create!(name: "d", note: "n4")
    out << "create: #{c.note} | #{c.persisted?} #{c.trace.inspect}"
    c.update(note: "n5", code: "v")
    out << "update: #{c.note} | #{c.trace.last}"
    t.remember(:code)
    Shop.find(t.id).update!(code: "z")
    out << "before reload: #{t.code} #{t.cached(:code)}"
    out << "reload: #{t.reload.equal?(t)} #{t.code} #{t.cached(:code).inspect} #{t.trace.last}"
    d = t.dup
    out << "dup: #{d.trace.last} cache=#{d.cache_size} #{t.trace.last}"
    out
  end
end
"##;

/// Native boot: Rails::Railtie from the pinned railties, then the gem as
/// Bundler.require loads it. Native only.
const NATIVE_BOOT: &str = r#"railties = $LOAD_PATH.grep(%r{/activerecord/lib\z}).first.sub(%r{/activerecord/lib\z}, "/railties/lib")
$LOAD_PATH.unshift(railties)
require "rails/railtie"
require_relative "vendor/gems/tracing-1.0.0/lib/tracing"
"#;

const EXPECTED: [&str; 14] = [
    r#"new: ["init_internals"] cache=true"#,
    r#"load: ["init_internals"] cache=true"#,
    r#"assign: nil n1 name="a" raw=[:note, :name] | b ["init_internals", "assign:note,name"] raw=nil"#,
    r#"strings: n2 name="c" raw=[:name, :note] | c"#,
    r#"attributes=: y assign:code"#,
    r#"empty: nil 4"#,
    r#"unknown: ActiveModel::UnknownAttributeError unknown attribute 'bogus' for Shop. bogus true true"#,
    r#"not a hash: When assigning attributes, you must pass a hash as an argument, String passed."#,
    r#"new attrs: n3 name=nil raw=[:note, :code] | w ["init_internals", "assign:note,code"]"#,
    r#"create: n4 name="d" raw=[:name, :note] | true ["init_internals", "assign:name,note"]"#,
    r#"update: n5 name="d" raw=[:note, :code] | assign:note,code"#,
    r#"before reload: y y"#,
    r#"reload: true z nil reload"#,
    r#"dup: dup cache=0 reload"#,
];

const FILES: [(&str, &str); 14] = [
    ("roundhouse.yml", ""),
    ("Gemfile", "source \"https://rubygems.org\"\n\ngem \"tracing\"\n"),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("vendor/gems/tracing-1.0.0/lib/tracing.rb", GEM_MAIN),
    ("vendor/gems/tracing-1.0.0/lib/tracing/railtie.rb", GEM_RAILTIE),
    ("vendor/gems/tracing-1.0.0/lib/tracing/hooks.rb", GEM_HOOKS),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/core_hooks_probe.rb", PROBE),
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
fn base_include_record_hooks_match_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-record-core-hooks",
        &FILES,
        &emitted,
        "rh_spec_active_record_core_hooks",
        "CoreHooksProbe",
        &["native_boot", "app/models/application_record", "app/models/shop", "app/models/core_hooks_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

const MACRO_HOOKS: &str = r##"module Tracing
  module Hooks
    def self.included(base)
      base.extend(ClassMethods)
    end

    def reload(*)
      @trace_cache = {}
      super
    end

    private

    def init_internals
      @trace_cache = {}
      super
    end

    module ClassMethods
      def traced(*columns)
        columns.flatten.each do |column|
          column_string = column.to_s.freeze
          define_method("#{column}_traced") do
            @trace_cache[column_string] ||= "#{column_string}:#{public_send(column_string)}"
          end
        end
      end
    end
  end
end
"##;

fn with_macro(hooks: &str) -> emit_and_run::Overlay {
    overlay()
        .write("vendor/gems/tracing-1.0.0/lib/tracing/hooks.rb", hooks)
        .write("app/models/shop.rb", "class Shop < ApplicationRecord\n  traced :code\nend\n")
        .write("app/models/core_hooks_probe.rb", "class CoreHooksProbe\nend\n")
}

/// A class macro of an include whose overrides the tree's Base ancestry
/// runs (`init_internals`, `reload`) is replayed.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_of_an_include_wrapping_init_internals_and_reload_is_replayed() {
    let (emitted, errors) = with_macro(MACRO_HOOKS).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).expect("shop.rb");
    assert!(shop.contains("traced(:code)") || shop.contains("traced :code"), "{shop}");
    let core = std::fs::read_to_string(emitted.join("runtime/active_record_core.rb")).expect("active_record_core.rb");
    assert!(core.contains("module Persistence"), "{core}");
}

/// An `_assign_attributes` override runs for `assign_attributes` and for
/// the compiled `new`/`create`/`update`, so a macro of its include is
/// replayed.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_of_an_include_wrapping_assign_attributes_is_replayed() {
    let hooks = MACRO_HOOKS.replace(
        "    module ClassMethods",
        "    def _assign_attributes(new_attributes)\n      @raw = new_attributes\n      super\n    ensure\n      @raw = nil\n    end\n\n    module ClassMethods",
    );
    let (emitted, errors) = with_macro(&hooks).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).expect("shop.rb");
    assert!(shop.contains("traced(:code)") || shop.contains("traced :code"), "{shop}");
}
