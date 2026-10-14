//! The binary opens the database `config/database.yml` names for
//! `RAILS_ENV`, as Rails does.
//!
//! It used to open `BLOG_DB` or `storage/development.sqlite3`, whatever
//! the environment: hindmost runs with RAILS_ENV=production and its file
//! is /data/web/production.sqlite3 (a multi-database environment whose
//! first entry is the primary).

use std::collections::HashMap;
use std::path::PathBuf;

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

fn boot_rb(database_yml: Option<&str>) -> String {
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
