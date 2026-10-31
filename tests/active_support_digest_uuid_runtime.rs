//! PostgreSQL: active support digest uuid runtime, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
//
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn digest_uuid_answers_like_rails() {
    let out = Command::new("ruby")
        .arg(root().join("tests/active_support_digest_uuid_runtime.rb"))
        .arg("compare")
        .current_dir(root())
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("Native Rails and emitted Digest::UUID agree"),
        "port diverged from Rails\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    for expected in [
        "v5 url: \"09cdf74e-b3df-52fc-bedf-5f14badd5d88\"",
        "v5 string namespace: \"79658267-b94c-588d-b978-56bb04956617\"",
        "bad namespace: \"ArgumentError: Only UUIDs are valid namespace identifiers\"",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
}

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

// Reviews::SafeRiskAssessmentEventId's shape.
const MODEL: &str = "class EventId\n  NAMESPACE = \"6ba7b810-9dad-11d1-80b4-00c04fd430c8\"\n\n  def self.generate(assessment_id, shop_id)\n    \
name = \"assessment-#{assessment_id}-#{shop_id}\"\n    Digest::UUID.uuid_v5(NAMESPACE, name)\n  end\n\n  def self.url(path) = ::Digest::UUID.uuid_v5(Digest::UUID::URL_NAMESPACE, path)\nend\n";

fn app(model: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_digest_uuid_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/event_id.rb", model)
}

/// App code naming Digest::UUID resolves it and loads the port at boot;
/// app code that does not, does not, though a gem's require of the
/// feature still finds it under runtime/features.
#[test]
#[ignore = "fork spec: postgresql"]
fn app_code_naming_digest_uuid_loads_the_port_at_boot() {
    let (tree, _) = app(MODEL).emit(BuildTarget::Ruby);
    let model = std::fs::read_to_string(tree.join("app/models/event_id.rb")).unwrap();
    assert!(!model.contains("not supported"), "{model}");
    assert!(model.contains("Digest::UUID.uuid_v5(") && model.contains("Digest::UUID::URL_NAMESPACE"), "{model}");
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(facades.contains("require_relative \"active_support_digest_uuid\"\n"), "port not loaded at boot:\n{facades}");
    assert!(tree.join("runtime/active_support_digest_uuid.rb").is_file());
    let feature = std::fs::read_to_string(tree.join("runtime/features/active_support/core_ext/digest/uuid.rb")).unwrap();
    assert!(feature.contains("active_support_digest_uuid"), "{feature}");

    let (tree, _) = app("class EventId\n  # Digest::UUID.uuid_v5\n  def self.generate(a) = a\nend\n").emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(!facades.contains("require_relative \"active_support_digest_uuid\""), "loaded without a read:\n{facades}");
}

/// The booted tree answers as native Rails does (the oracle's values).
/// Boot loads the `pg` gem (it never connects): run with `--ignored` and
/// a `GEM_PATH` that has it.
#[test]
#[ignore = "fork spec: postgresql"]
fn booted_tree_generates_rails_uuids() {
    let (tree, _) = app(MODEL).emit(BuildTarget::Ruby);
    let output = Command::new("ruby")
        .args(["-e", "require \"./boot\"\nputs EventId.generate(1, 42)\nputs EventId.url(\"https://example.com/hooks/1\")\n"])
        .current_dir(&*tree)
        .env_remove("DATABASE_URL")
        .env_remove("ROUNDHOUSE_DATABASE_URL")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(stdout, "79658267-b94c-588d-b978-56bb04956617\n09cdf74e-b3df-52fc-bedf-5f14badd5d88\n", "stderr={stderr}");
}
