//! PostgreSQL: active support securerandom runtime, on the Ruby target against native Rails.
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
fn securerandom_answers_like_rails() {
    let out = Command::new("ruby")
        .arg(root().join("tests/active_support_securerandom_runtime.rb"))
        .arg("compare")
        .current_dir(root())
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("Native Rails and emitted SecureRandom.base58 agree"),
        "port diverged from Rails\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    for expected in [
        "base58 alphabet: \"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz\"",
        "base58(24): \"cseEjiC1fR4umh7ko7UqgRtW\"",
        "default lengths: [16, 16, 16]",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
}

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

// AccessAndAuth's recognition test setup and BulkOperationAsset's shape.
const MODEL: &str = "class PublicKey\n  def self.public_id = SecureRandom.base58(24)\n\n  \
def self.object_name(prefix) = \"#{prefix}/#{::SecureRandom.base36(28)}\"\nend\n";

fn app(model: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_securerandom_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/public_key.rb", model)
}

/// App code calling SecureRandom.base58 loads the port at boot; app code
/// that does not, does not, though a gem's require of the feature still
/// finds it under runtime/features.
#[test]
#[ignore = "fork spec: postgresql"]
fn app_code_calling_base58_loads_the_port_at_boot() {
    let (tree, _) = app(MODEL).emit(BuildTarget::Ruby);
    let model = std::fs::read_to_string(tree.join("app/models/public_key.rb")).unwrap();
    assert!(!model.contains("not supported"), "{model}");
    assert!(model.contains("SecureRandom.base58(24)") && model.contains("SecureRandom.base36(28)"), "{model}");
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(facades.contains("require_relative \"active_support_securerandom\"\n"), "port not loaded at boot:\n{facades}");
    assert!(tree.join("runtime/active_support_securerandom.rb").is_file());
    let feature = std::fs::read_to_string(tree.join("runtime/features/active_support/core_ext/securerandom.rb")).unwrap();
    assert!(feature.contains("active_support_securerandom"), "{feature}");

    let (tree, _) = app("class PublicKey\n  # SecureRandom.base58\n  def self.public_id = SecureRandom.hex(8)\nend\n").emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(!facades.contains("require_relative \"active_support_securerandom\""), "loaded without a call:\n{facades}");
}

/// The booted tree answers base58/base36 as Rails does: the lengths and
/// alphabets of the oracle. Boot loads the `pg` gem (it never connects):
/// run with `--ignored` and a `GEM_PATH` that has it.
#[test]
#[ignore = "fork spec: postgresql"]
fn booted_tree_generates_base58() {
    let (tree, _) = app(MODEL).emit(BuildTarget::Ruby);
    let script = "require \"./boot\"\nid = PublicKey.public_id\nputs id.length, id.match?(/\\A[1-9A-HJ-NP-Za-km-z]+\\z/)\n\
                  puts PublicKey.object_name(\"p\").match?(%r{\\Ap/[0-9a-z]{28}\\z})\n";
    let output = Command::new("ruby")
        .args(["-e", script])
        .current_dir(&*tree)
        .env_remove("DATABASE_URL")
        .env_remove("ROUNDHOUSE_DATABASE_URL")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(stdout, "24\ntrue\ntrue\n", "stderr={stderr}");
}
