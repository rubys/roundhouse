//! The app's data files reach the emitted tree at their Rails.root-
//! relative paths, so app code that reads one by a path it builds runs
//! as it does on the source tree. a large app's `RegionDefaults`
//! loads `components/regions/config/countries_for_currency_overrides.yml`
//! in its class body; the emitted tree had no data files at all, and
//! boot died with Errno::ENOENT.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_class_body_reads_a_data_file_under_rails_root() {
    let run = emit_and_run::empty_app()
        .write("components/regions/config/overrides.yml", "EUR:\n  - FR\n  - DE\n")
        .write("db/data/fonts.json", "{\"serif\": [\"Georgia\"]}\n")
        .write("lib/templates/page.yml.erb", "title: <%= 1 + 1 %>\n")
        .write(
            "lib/region_defaults.rb",
            "require \"yaml\"\nrequire \"json\"\nrequire \"erb\"\n\nclass RegionDefaults\n  OVERRIDES = YAML.safe_load_file(File.join(Rails.root, \"components\", \"regions\", \"config\", \"overrides.yml\")).freeze\n  FONTS = JSON.parse(File.read(File.join(Rails.root, \"db\", \"data\", \"fonts.json\"))).freeze\n  PAGE = YAML.safe_load(ERB.new(File.read(File.join(Rails.root, \"lib\", \"templates\", \"page.yml.erb\"))).result).freeze\nend\n",
        )
        .run_ruby("p RegionDefaults::OVERRIDES\np RegionDefaults::FONTS\np RegionDefaults::PAGE\n");
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "{\"EUR\" => [\"FR\", \"DE\"]}\n{\"serif\" => [\"Georgia\"]}\n{\"title\" => 2}\n");
}

#[test]
fn credentials_keys_and_type_declarations_are_not_carried() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("config/data.yml", "a: 1\n")
        .write("config/credentials.yml", "secret: x\n")
        .write("config/credentials/test.yml", "secret: x\n")
        .write("config/master.key", "x\n")
        .write("config/settings.ejson", "{}\n")
        .write("config/database.yml", "test:\n  adapter: sqlite3\n  database: \":memory:\"\n")
        .write("sorbet/config.yml", "a: 1\n")
        .write("vendor/gems/g/data.yml", "a: 1\n")
        .write("lib/thing.rb", "class Thing\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(emitted.join("config/data.yml").is_file());
    for path in [
        "config/credentials.yml",
        "config/credentials/test.yml",
        "config/master.key",
        "config/settings.ejson",
        "config/database.yml",
        "sorbet/config.yml",
        "vendor/gems/g/data.yml",
    ] {
        assert!(!emitted.join(path).exists(), "{path} was carried");
    }
}
