//! PostgreSQL: active record fixture set, on the Ruby target against native Rails.
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
  database: rh_spec_active_record_fixture_set
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = "class Shop < ApplicationRecord\nend\n";

/// FasterFixtures::FixtureFile's shape: a `table_name` key in `_fixture`.
const PROBE_FILE: &str = r##"class ProbeFixtureFile < ActiveRecord::FixtureSet::File
  def table_name
    config_row["table_name"]
  end

  private

  def validate_config_row(data)
    unless Hash === data
      raise ActiveRecord::Fixture::FormatError, "Invalid `_fixture` section: `_fixture` must be a hash: #{@file}"
    end

    begin
      data.assert_valid_keys("model_class", "ignore", "table_name")
    rescue ArgumentError => error
      raise ActiveRecord::Fixture::FormatError, "Invalid `_fixture` section: #{error.message}: #{@file}"
    end

    data
  end
end
"##;

const SHOPS_YML: &str = "_fixture:\n  model_class: Shop\n  table_name: shops\n  ignore: DEFAULTS\nDEFAULTS: &DEFAULTS\n  code: d\nsnow:\n  name: <%= \"snow\".upcase %>\n  <<: *DEFAULTS\nrain:\n  name: rain\n";

const PROBE: &str = r##"require "tmpdir"

class FixtureSetProbe
  def self.lines
    out = []
    out << "identify: #{ActiveRecord::FixtureSet.identify("foo")} #{ActiveRecord::FixtureSet.identify(:foo, :integer)} #{ActiveRecord::FixtureSet::MAX_ID}"
    out << "uuid: #{ActiveRecord::FixtureSet.identify("foo", :uuid)} #{ActiveRecord::FixtureSet.identify(:snowdevil, :uuid)}"
    ids = ActiveRecord::FixtureSet.composite_identify("foo", [:shop_id, :id])
    out << "composite: #{ids.to_h.inspect} #{ids[:id]}"
    out << "names: #{ActiveRecord::FixtureSet.default_fixture_model_name("shops")} #{ActiveRecord::FixtureSet.default_fixture_table_name("admin/shops").inspect}"
    dir = Dir.mktmpdir
    path = File.join(dir, "shops.yml")
    File.write(path, SHOPS)
    ProbeFixtureFile.open(path) do |fh|
      out << "file: #{fh.model_class} #{fh.table_name} #{fh.ignored_fixtures.inspect} #{fh.map(&:first).inspect}"
      row = fh.to_a.first.last
      out << "rows: #{row.inspect} #{row.frozen?}"
    end
    bad = File.join(dir, "bad.yml")
    File.write(bad, "_fixture:\n  table: x\n")
    begin
      ProbeFixtureFile.new(bad).model_class
    rescue ActiveRecord::Fixture::FormatError => e
      out << "format: #{e.class} #{e.message.sub(dir, "DIR")} #{e.is_a?(ActiveRecord::Fixture::FixtureError)}"
    end
    shop = Shop.create!(name: "x")
    fixture = ActiveRecord::Fixture.new({ "id" => shop.id, "name" => "x" }, Shop)
    out << "find: #{fixture.find.name} #{fixture.find.id == shop.id} #{fixture.class_name} #{fixture["name"]} #{fixture.to_hash.keys.inspect}"
    begin
      ActiveRecord::Fixture.new({}, nil).find
    rescue ActiveRecord::FixtureClassNotFound => e
      out << "not found: #{e.message} #{e.is_a?(ActiveRecord::ActiveRecordError)}"
    end
    ActiveRecord::FixtureSet.reset_cache
    ActiveRecord::FixtureSet.cache_fixtures(:pool, { "shops" => 1 })
    out << "cache: #{ActiveRecord::FixtureSet.fixture_is_cached?(:pool, "shops").inspect} #{ActiveRecord::FixtureSet.fixture_is_cached?(:other, "shops").inspect} #{ActiveRecord::FixtureSet.cached_fixtures(:pool, ["shops", "x"]).inspect} #{ActiveRecord::FixtureSet.cached_fixtures(:pool).inspect}"
    ActiveRecord::FixtureSet.reset_cache
    out << "reset: #{ActiveRecord::FixtureSet.cached_fixtures(:pool).inspect}"
    ActiveRecord::FixtureSet.all_loaded_fixtures["probe"] ||= []
    ActiveRecord::FixtureSet.all_loaded_fixtures["probe"] << 1
    out << "loaded: #{ActiveRecord::FixtureSet.all_loaded_fixtures["probe"].inspect}"
    begin
      ActiveRecord::FixtureSet::File.new(path).model_class
    rescue ActiveRecord::Fixture::FormatError => e
      out << "base file: #{e.message.sub(dir, "DIR")}"
    end
    File.write(File.join(dir, "plain.yml"), SHOPS.sub("  table_name: shops\n", ""))
    set = ActiveRecord::FixtureSet.new(nil, "shops", "Shop", File.join(dir, "plain"))
    out << "set: #{set.table_name} #{set.model_class} #{set.size} #{set["snow"]["name"]} #{set["rain"].class} #{set.ignored_fixtures.inspect}"
    out << "context: #{ActiveRecord::FixtureSet.context_class.equal?(ActiveRecord::FixtureSet.context_class)} #{ActiveRecord::FixtureSet::RenderContext.create_subclass.superclass.equal?(ActiveRecord::FixtureSet.context_class)}"
    out
  end
end
"##;

/// Native boot: Active Record autoloads ActiveRecord::FixtureSet. Native
/// only.
const NATIVE_BOOT: &str = "require \"active_record/fixtures\"\n";

const EXPECTED: [&str; 15] = [
    "identify: 208889123 208889123 1073741823",
    "uuid: bca95adb-b5f1-564f-96a7-6355c52d1fa7 dac24f35-58f1-52a5-b8fc-ed3048d1fd21",
    r#"composite: {"shop_id" => 208889123, "id" => 417778246} 417778246"#,
    "names: Shop :admin_shops",
    r#"file: Shop shops "DEFAULTS" ["DEFAULTS", "snow", "rain"]"#,
    r#"rows: {"code" => "d"} true"#,
    "format: ActiveRecord::Fixture::FormatError Invalid `_fixture` section: Unknown key: \"table\". Valid keys are: \"model_class\", \"ignore\", \"table_name\": DIR/bad.yml true",
    r#"find: x true Shop x ["id", "name"]"#,
    "not found: No class attached to find. true",
    r#"cache: 1 nil [1, nil] [1]"#,
    "reset: []",
    "loaded: [1]",
    "base file: Invalid `_fixture` section: Unknown key: \"table_name\". Valid keys are: \"model_class\", \"ignore\": DIR/shops.yml",
    r#"set: shops Shop 3 SNOW ActiveRecord::Fixture ["DEFAULTS"]"#,
    "context: true true",
];

fn files() -> Vec<(&'static str, String)> {
    vec![
        ("roundhouse.yml", "".into()),
        ("config/database.yml", DATABASE_YML.into()),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n".into()),
        ("db/structure.sql", STRUCTURE.into()),
        ("app/models/application_record.rb", APPLICATION_RECORD.into()),
        ("app/models/shop.rb", SHOP.into()),
        ("app/models/probe_fixture_file.rb", PROBE_FILE.into()),
        ("app/models/fixture_set_probe.rb", PROBE.replace("SHOPS", &format!("{SHOPS_YML:?}"))),
        ("native_boot.rb", NATIVE_BOOT.into()),
    ]
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn app_fixture_code_over_fixture_set_matches_native_on_postgresql() {
    let files = files();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let app = refs.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-record-fixture-set",
        &refs,
        &emitted,
        "rh_spec_active_record_fixture_set",
        "FixtureSetProbe",
        &["native_boot", "app/models/application_record", "app/models/shop", "app/models/probe_fixture_file", "app/models/fixture_set_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_postgresql_tree_carries_fixture_set_before_app_files() {
    let files = files();
    let app = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let port = std::fs::read_to_string(emitted.join("runtime/active_record_fixture_set.rb")).expect("fixture set port");
    assert!(port.contains("class FixtureSet") && port.contains("class File"), "{port}");
    let facades = std::fs::read_to_string(emitted.join("runtime/gem_facades.rb")).expect("gem_facades.rb");
    assert!(facades.contains("require_relative \"active_record_fixture_set\"\n"), "{facades}");
    let file = std::fs::read_to_string(emitted.join("app/models/probe_fixture_file.rb")).expect("probe_fixture_file.rb");
    assert!(file.contains("< ActiveRecord::FixtureSet::File"), "{file}");
    assert!(file.contains("ActiveRecord::Fixture::FormatError"), "{file}");
    assert!(!file.contains("constant not supported"), "{file}");
}
