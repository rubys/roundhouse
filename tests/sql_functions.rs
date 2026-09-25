//! SQL functions an initializer registers on SQLite connections.
//!
//! Lobsters' `config/initializers/sqlite_functions.rb` patches the
//! SQLite adapter so every connection carries `regexp`, `if` and the
//! `stddev` aggregate; its flag statistics call `stddev(…)` in raw SQL,
//! reached from any page that shows your own threads. Without them the
//! query fails with "no such function".
//!
//! Ingest reads each `create_function` / `create_aggregate` block into
//! an `App::sql_functions` entry whose bodies are ordinary methods, and
//! the CRuby tree installs them through the sqlite3 gem on every pooled
//! connection (`runtime/sql_functions.rb`, called from `Db.open_pool`).

use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

const INITIALIZER: &str = r#"require "active_record/connection_adapters/sqlite3_adapter"

ActiveSupport.on_load(:active_record) do
  ActiveRecord::ConnectionAdapters::SQLite3Adapter.class_eval do
    alias_method :orig_initialize, :initialize

    def initialize(connection, logger = nil, pool = nil)
      orig_initialize(connection, logger, pool)

      raw_connection.create_function("regexp", 2) do |fn, pattern, expr|
        matcher = Regexp.new(pattern.to_s, Regexp::IGNORECASE)
        fn.result = expr.to_s.match(matcher) ? 1 : 0
      end

      raw_connection.create_function("if", 3) do |fn, c, t, f|
        fn.result = (c == 1) ? t : f
      end

      raw_connection.create_aggregate("stddev", 1) do
        step do |fn, value|
          next if value.nil?

          fn[:n] ||= 0
          fn[:sum_of_squares] ||= 0
          fn[:sum] ||= 0

          fn[:n] += 1
          fn[:sum_of_squares] += value**2
          fn[:sum] += value
        end

        finalize do |fn|
          fn.result = if fn[:n].nil?
            nil
          else
            Math.sqrt((fn[:sum_of_squares].to_f / fn[:n]) - ((fn[:sum].to_f / fn[:n])**2))
          end
        end
      end
    end
  end
end
"#;

fn fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-sql-functions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("config/initializers/sqlite_functions.rb", INITIALIZER),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    dir
}

fn installer() -> String {
    let dir = fixture();
    let mut app = ingest_app(&dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let names: Vec<(&str, usize)> =
        app.sql_functions.iter().map(|f| (f.name.as_str(), f.arity)).collect();
    assert_eq!(names, [("regexp", 2), ("if", 3), ("stddev", 1)]);
    let files = roundhouse::project::target_files(&app, &dir, BuildTarget::Ruby).expect("files");
    let db = files.iter().find(|(p, _)| p == "runtime/db.rb").expect("db.rb");
    assert!(db.1.starts_with("require_relative \"sql_functions\""), "db.rb loads it");
    files
        .into_iter()
        .find(|(p, _)| p == "runtime/sql_functions.rb")
        .map(|(_, c)| c)
        .expect("runtime/sql_functions.rb")
}

#[test]
fn the_blocks_become_methods_and_an_installer() {
    let src = installer();
    // `next` leaves the block; in the method it is a `return`.
    assert!(src.contains("def self.agg_stddev_step(fn, value)\n    return if value.nil?"), "{src}");
    assert!(src.contains("def self.fn_regexp(fn, pattern, expr)"), "{src}");
    assert!(src.contains("db.create_function(\"if\", 3)"), "{src}");
    assert!(src.contains("db.create_aggregate(\"stddev\", 1) do"), "{src}");
}

#[test]
fn the_installed_functions_answer_what_the_apps_blocks_would() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let dir = std::env::temp_dir().join(format!("roundhouse-sql-functions-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sql_functions.rb"), installer()).unwrap();
    let out = Command::new("ruby")
        .arg("-rsqlite3")
        .arg("-e")
        .arg(
            r#"require_relative ARGV[0]
db = SQLite3::Database.new(":memory:")
SqlFunctions.install(db)
p db.execute("select stddev(x) from (select 2 as x union all select 4 union all select 4 union all select 4 union all select 5 union all select 5 union all select 7 union all select 9)")[0][0]
p db.execute("select regexp('^ab', 'ABC'), regexp('^x', 'abc')")[0]
p db.execute("select if(1, 'yes', 'no'), if(0, 'yes', 'no')")[0]
p db.execute("select stddev(null)")[0][0]"#,
        )
        .arg(dir.join("sql_functions.rb"))
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "2.0\n[1, 0]\n[\"yes\", \"no\"]\nnil\n");
}
