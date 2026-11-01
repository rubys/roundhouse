//! The binary opens the database `config/database.yml` names for
//! `RAILS_ENV`, as Rails does.
//!
//! It used to open `BLOG_DB` or `storage/development.sqlite3`, whatever
//! the environment: an app run with RAILS_ENV=production whose file is
//! /data/web/production.sqlite3 (a multi-database environment whose
//! first entry is the primary) opened the development database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::project::BuildTarget;

const DATABASE_YML: &str = r#"default: &default
  adapter: sqlite3
  max_connections: <%= ENV.fetch("RAILS_MAX_THREADS") { 5 } %>
  timeout: 5000

development:
  <<: *default
  database: storage/development.sqlite3

test:
  <<: *default
  database: storage/test.sqlite3

staging:
  <<: *default
  database: <%= ENV["STAGING_DB"] %>

production:
  primary:
    <<: *default
    database: /data/web/production.sqlite3
  cache:
    <<: *default
    database: /data/web/production_cache.sqlite3
"#;

fn ingest(database_yml: Option<&str>) -> roundhouse::App {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
    ];
    if let Some(yml) = database_yml {
        files.push(("config/database.yml", yml));
    }
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn boot_rb(database_yml: Option<&str>) -> String {
    let app = ingest(database_yml);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    files.into_iter().find(|(p, _)| p == "boot.rb").map(|(_, c)| c).expect("boot.rb")
}

#[test]
fn each_environment_names_its_main_database() {
    let boot = boot_rb(Some(DATABASE_YML));
    assert!(boot.contains("module AppDatabase"), "{boot}");
    assert!(boot.contains("when \"development\" then \"storage/development.sqlite3\""), "{boot}");
    assert!(boot.contains("when \"test\" then \"storage/test.sqlite3\""), "{boot}");
    assert!(
        boot.contains("when \"production\" then \"/data/web/production.sqlite3\""),
        "the first entry of a multi-database environment is the primary:\n{boot}"
    );
    assert!(!boot.contains("production_cache"), "{boot}");
}

#[test]
fn a_computed_database_path_is_left_to_the_default() {
    let boot = boot_rb(Some(DATABASE_YML));
    assert!(!boot.contains("\"staging\""), "an ERB database path is Rails' to compute at boot:\n{boot}");
}

#[test]
fn an_app_without_database_yml_still_gets_the_lookup() {
    let boot = boot_rb(None);
    assert!(boot.contains("module AppDatabase\n  def self.path_for(env)\n    nil\n  end\nend"), "{boot}");
}

/// Campfire names `storage/db/development.sqlite3`, and a fresh checkout
/// has no `storage/db/`. Rails' SQLite3Adapter `mkdir_p`s the database
/// file's directory before opening it; the emitted app does the same, in
/// `SqliteAdapter.configure`, which both lanes call.
#[test]
fn the_database_directory_is_created_as_rails_creates_it() {
    let app = ingest(Some(
        "development:\n  adapter: sqlite3\n  database: storage/db/development.sqlite3\n",
    ));
    let files = roundhouse::project::target_files(&app, Path::new("."), BuildTarget::Ruby)
        .expect("Ruby project");
    let root = std::env::temp_dir().join(format!("roundhouse-database-yml-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    roundhouse::project::write_to_dir(&files, &root).unwrap();
    assert!(!root.join("storage/db").exists(), "the emitted tree must not ship storage/db/");

    // The server's path: RAILS_ENV=development, no BLOG_DB, the file
    // database.yml names. Then an absolute path two directories deep, and
    // the in-memory names, which name no directory.
    let script = r#"
require_relative "main"
Main.configure_default_adapter!
SqliteAdapter.execute_ddl("CREATE TABLE probes (id INTEGER)")
raise "no storage/db/development.sqlite3" unless File.file?("storage/db/development.sqlite3")
nested = File.join(Dir.pwd, "deep", "er", "nested.sqlite3")
SqliteAdapter.create_parent_directory(nested)
raise "no #{File.dirname(nested)}" unless Dir.exist?(File.dirname(nested))
SqliteAdapter.create_parent_directory(":memory:")
SqliteAdapter.create_parent_directory("file:shared/db?mode=memory&cache=shared")
raise "a file: URI made a directory" if Dir.exist?("file:shared")
puts "database directories created"
"#;
    let output = Command::new("ruby")
        .current_dir(&root)
        .env_remove("BLOG_DB")
        .env("RAILS_ENV", "development")
        .args(["-e", script])
        .output()
        .expect("run ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("database directories created"),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}
