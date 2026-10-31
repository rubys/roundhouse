//! PostgreSQL: multi database, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  primary:
    <<: *default
    database: rh_spec_mdb_primary
  primary_replica:
    <<: *default
    database: rh_spec_mdb_primary
    replica: true
  animals:
    <<: *default
    database: rh_spec_mdb_animals
    migrations_paths: db/animals_migrate
  animals_replica:
    <<: *default
    database: rh_spec_mdb_animals
    replica: true
"#;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "posts", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.text "body", null: false
    t.bigint "post_id", null: false
  end
  create_table "dogs", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A fresh directory under the test target's scratch space, kept after
/// the run so a failure can be inspected.
fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("multi-db-pg-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// tiny-blog with the two abstract classes, Dog under AnimalsRecord, the
/// dogs table, `roundhouse.yml` (postgresql) and the four configurations.
fn fixture_app() -> PathBuf {
    fixture_app_in("app")
}

fn fixture_app_in(tag: &str) -> PathBuf {
    let app = scratch(tag);
    copy_tree(&Path::new(MANIFEST_DIR).join("fixtures/tiny-blog"), &app);
    let write = |rel: &str, text: &str| {
        let path = app.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  connects_to database: { writing: :primary, reading: :primary_replica }\nend\n",
    );
    write(
        "app/models/animals_record.rb",
        "class AnimalsRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  connects_to database: { writing: :animals, reading: :animals_replica }\nend\n",
    );
    write("app/models/dog.rb", "class Dog < AnimalsRecord\n  validates :name, presence: true\nend\n");
    write("db/schema.rb", SCHEMA);
    write("roundhouse.yml", "");
    write("config/database.yml", DATABASE_YML);
    app
}

fn emit(app: &roundhouse::app::App) -> PathBuf {
    emit_in(app, "out")
}

fn emit_in(app: &roundhouse::app::App, tag: &str) -> PathBuf {
    let out = scratch(tag);
    let files = roundhouse::project::target_files(
        app,
        &Path::new(MANIFEST_DIR).join("fixtures/tiny-blog"),
        roundhouse::project::BuildTarget::Ruby,
    )
    .expect("package PostgreSQL Ruby project");
    for (name, content) in files {
        let path = out.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    out
}

fn read(out: &Path, rel: &str) -> String {
    std::fs::read_to_string(out.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The compiler side: both `connects_to` are owned (no refusal), replayed
/// at boot, and the abstract classes stay abstract in the emitted tree.
#[test]
#[ignore = "fork spec: postgresql"]
fn connects_to_on_two_abstract_classes_compiles_without_refusal() {
    let app_dir = fixture_app();
    let app = roundhouse::ingest::ingest_app(&app_dir).expect("ingest probe app");
    // `database_adapter` is the check-time note that only the CRuby
    // target carries a PostgreSQL driver (as in tests/database_yml_boot.rs).
    let refusals: Vec<_> = app.database_refusals.iter().filter(|r| r.construct != "database_adapter").collect();
    assert!(refusals.is_empty(), "{refusals:?}");
    let out = emit(&app);
    let configuration = read(&out, "runtime/database_configuration.rb");
    assert!(
        configuration.contains(r#"OWNERSHIP = [["AnimalsRecord", "animals", "animals_replica"], ["ApplicationRecord", "primary", "primary_replica"]]"#)
            || configuration.contains(r#"OWNERSHIP = [["ApplicationRecord", "primary", "primary_replica"], ["AnimalsRecord", "animals", "animals_replica"]]"#),
        "{configuration}"
    );
    assert!(configuration.contains("ActiveRecord::Base.establish_connection"), "{configuration}");
    assert!(out.join("runtime/active_record_connection_handling.rb").exists());
    for model in ["app/models/application_record.rb", "app/models/animals_record.rb"] {
        let text = read(&out, model);
        assert!(text.contains("self.abstract_class = true"), "{model}: {text}");
    }
}

const CONDITIONAL_ANIMALS_RECORD: &str = r#"class AnimalsRecord < ActiveRecord::Base
  self.abstract_class = true

  ENABLED = !Rails.env.test? || ENV["DISABLE_ANIMALS"].blank?

  if ENABLED
    animals_config = configurations.resolve(:animals)
    computed = ActiveRecord::DatabaseConfigurations::HashConfig.new(
      animals_config.env_name,
      animals_config.name,
      animals_config.configuration_hash.merge(application_name: "animals-probe"),
    )
    connects_to database: { writing: computed, reading: :animals_replica }
  end
end
"#;

/// A `connects_to` the program runs itself (under a condition, from
/// computed arguments, `shards:`) is neither refused nor dropped: the
/// emitted class body runs it as written, and its models are not in the
/// boot replay.
#[test]
#[ignore = "fork spec: postgresql"]
fn conditional_connects_to_is_replayed_in_the_class_body() {
    let app_dir = fixture_app_in("cond-app");
    std::fs::write(app_dir.join("app/models/animals_record.rb"), CONDITIONAL_ANIMALS_RECORD).unwrap();
    let app = roundhouse::ingest::ingest_app(&app_dir).expect("ingest probe app");
    let refusals: Vec<_> = app.database_refusals.iter().filter(|r| r.construct != "database_adapter").collect();
    assert!(refusals.is_empty(), "{refusals:?}");
    let out = emit_in(&app, "cond-out");
    let configuration = read(&out, "runtime/database_configuration.rb");
    assert!(configuration.contains(r#"OWNERSHIP = [["ApplicationRecord", "primary", "primary_replica"]]"#), "{configuration}");
    assert!(configuration.contains("UNRESOLVED = {  }") || configuration.contains("UNRESOLVED = {}"), "{configuration}");
    let animals = read(&out, "app/models/animals_record.rb");
    assert!(animals.contains("if AnimalsRecord::ENABLED") || animals.contains("if ENABLED"), "{animals}");
    assert!(animals.contains("computed = "), "{animals}");
    assert!(animals.contains("connects_to database: { writing: computed, reading: :animals_replica }"), "{animals}");

    // `shards:` and a computed `database:` at the top of the body.
    std::fs::write(
        app_dir.join("app/models/animals_record.rb"),
        "class AnimalsRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  connects_to shards: { default: { writing: :animals, reading: :animals_replica } }\nend\n",
    )
    .unwrap();
    let app = roundhouse::ingest::ingest_app(&app_dir).expect("ingest probe app");
    let refusals: Vec<_> = app.database_refusals.iter().filter(|r| r.construct != "database_adapter").collect();
    assert!(refusals.is_empty(), "{refusals:?}");
    let out = emit_in(&app, "shards-out");
    let animals = read(&out, "app/models/animals_record.rb");
    assert!(animals.contains("connects_to shards: {"), "{animals}");
}

/// The conditional `connects_to` on the emitted program against local
/// PostgreSQL: enabled (Dog on the animals database, through a computed
/// HashConfig), disabled (never connected, Dog on primary), and with no
/// endpoint for the animals database (Rails' ConnectionNotEstablished).
/// Also the query cache predicates the runtime lacked.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn conditional_connects_to_runs_on_postgresql() {
    assert!(std::env::var_os("ROUNDHOUSE_PG_TEST_URL").is_some(), "set ROUNDHOUSE_PG_TEST_URL");
    let app_dir = fixture_app_in("cond-run-app");
    std::fs::write(app_dir.join("app/models/animals_record.rb"), CONDITIONAL_ANIMALS_RECORD).unwrap();
    std::fs::create_dir_all(app_dir.join("config/environments")).unwrap();
    std::fs::write(
        app_dir.join("config/environments/test.rb"),
        "Rails.application.configure do\n  config.eager_load = false\nend\n",
    )
    .unwrap();
    let app = roundhouse::ingest::ingest_app(&app_dir).expect("ingest probe app");
    let out = emit_in(&app, "cond-run-out");
    let result = Command::new("ruby")
        .arg("tests/conditional_connects_to_pg_runtime.rb")
        .arg(&out)
        .current_dir(MANIFEST_DIR)
        .output()
        .expect("ruby");
    println!("{}", String::from_utf8_lossy(&result.stdout));
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

/// The scenario on the emitted program and on native Rails, same PG.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn multiple_databases_route_like_native_rails_on_postgresql() {
    assert!(std::env::var_os("ROUNDHOUSE_PG_TEST_URL").is_some(), "set ROUNDHOUSE_PG_TEST_URL");
    let app_dir = fixture_app();
    let app = roundhouse::ingest::ingest_app(&app_dir).expect("ingest probe app");
    let out = emit(&app);
    let result = Command::new("ruby")
        .arg("tests/multi_database_pg_runtime.rb")
        .arg(&app_dir)
        .arg(&out)
        .current_dir(MANIFEST_DIR)
        .output()
        .expect("ruby");
    // The answers both sides gave, for the run log.
    println!("{}", String::from_utf8_lossy(&result.stdout));
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
