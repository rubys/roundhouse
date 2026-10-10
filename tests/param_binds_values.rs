//! Float/temporal read contract for roundhouse#12. Values serialize the same
//! way before inline escaping or binding. Run each compiler flag in its own
//! process, like param_binds.rs.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::process::Command;

// These probes run on the three supported local runtimes. Pin their default
// independently of the production capability table, then inspect actual emit.
fn expected_binds(target: BuildTarget) -> bool {
    match std::env::var("ROUNDHOUSE_PARAM_BINDS").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => target == BuildTarget::Spinel,
    }
}

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "sig/reading.rbs",
            "class Reading\n  def time_matches_value: (Time value) -> Integer\n  def required_time_matches_value: (Time? value) -> Integer\n  def required_ratio_matches_value: (Float? value) -> Integer\n  def string_time_matches_value: (String? value) -> Integer\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "readings", force: :cascade do |t|
    t.float "ratio", null: false
    t.float "optional_ratio"
    t.datetime "recorded_at", null: false
    t.datetime "optional_at"
  end
end
"#,
        )
        .write(
            "app/models/reading.rb",
            r#"
class Reading < ApplicationRecord
  def ratio_matches
    @number = ratio.to_f
    Reading.where(ratio: @number).count
  end
  def optional_ratio_matches
    Reading.where(optional_ratio: @optional_ratio).count
  end
  def scalar_optional_ratio_matches
    @number = ratio.to_f
    Reading.where(optional_ratio: @number).count
  end
  def required_ratio_matches_value(value)
    @nullable_number = value
    Reading.where(ratio: @nullable_number).count
  end
  def time_matches
    @needle = recorded_at
    Reading.where(recorded_at: @needle).count
  end
  def optional_time_matches
    @optional_needle = optional_at
    Reading.where(optional_at: @optional_needle).count
  end
  def time_matches_value(value)
    @bound_time = value
    Reading.where(recorded_at: @bound_time).count
  end
  def required_time_matches_value(value)
    @nullable_time = value
    Reading.where(recorded_at: @nullable_time).count
  end
  def string_time_matches
    @string_needle = "2023-11-14 22:13:20.123456"
    Reading.where(recorded_at: @string_needle).count
  end
  def string_time_matches_value(value)
    @optional_string_needle = value
    Reading.where(optional_at: @optional_string_needle).count
  end
  def pair_matches
    @key = id.to_i
    @number = ratio.to_f
    @needle = recorded_at
    Reading.where(id: @key, ratio: @number, recorded_at: @needle).count
  end
end
"#,
        )
}

fn success(command: &mut Command) {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

fn emitted(test: &str, target: BuildTarget) {
    if std::env::var_os("ROUNDHOUSE_BINDS_CHILD").is_none() {
        for mode in [None, Some("0"), Some("1")] {
            println!("{test}: ROUNDHOUSE_PARAM_BINDS={mode:?}");
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", test, "--include-ignored", "--nocapture"])
                .env("ROUNDHOUSE_BINDS_CHILD", "1")
                .env_remove("ROUNDHOUSE_PARAM_BINDS");
            if let Some(mode) = mode {
                child.env("ROUNDHOUSE_PARAM_BINDS", mode);
            }
            success(&mut child);
        }
        return;
    }
    let (dir, errors) = overlay().emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(dir.join("app/models/reading.rb")).unwrap();
    for method in [
        "ratio_matches",
        "optional_ratio_matches",
        "scalar_optional_ratio_matches",
        "required_ratio_matches_value(value)",
        "time_matches",
        "optional_time_matches",
        "time_matches_value(value)",
        "required_time_matches_value(value)",
        "string_time_matches",
        "string_time_matches_value(value)",
        "pair_matches",
    ] {
        let header = format!("  def {method}\n");
        let body = source
            .split_once(&header)
            .unwrap()
            .1
            .split_once("\n  end")
            .unwrap()
            .0;
        assert!(body.contains("Db.prepare("), "{header}{body}");
        assert!(!body.contains("Reading.where("), "{header}{body}");
        if method.starts_with("optional_") && expected_binds(target) {
            assert!(body.contains(" IS NULL"), "{header}{body}");
            assert!(body.contains(" = ?"), "{header}{body}");
            assert!(!body.contains(" IS ?"), "{header}{body}");
            assert!(body.contains("Db.bind_text_opt("), "{header}{body}");
        }
        if method == "ratio_matches" || method == "scalar_optional_ratio_matches" {
            assert!(body.contains("@number.to_s"), "{header}{body}");
            let bind = if method == "ratio_matches" { "Db.bind_text(" } else { "Db.bind_text_opt(" };
            assert_eq!(
                body.contains(bind),
                expected_binds(target),
                "{header}{body}"
            );
        }
        if method == "required_ratio_matches_value(value)" {
            assert!(body.contains(".to_s"), "nullable Float arguments must serialize before binding: {header}{body}");
        }
        if method.starts_with("string_time") {
            assert!(!body.contains("ActiveSupport.format_db_time("), "{header}{body}");
            let bind = if method == "string_time_matches" { "Db.bind_text(" } else { "Db.bind_text_opt(" };
            assert_eq!(body.contains(bind), expected_binds(target), "{header}{body}");
        } else if method.contains("time") || method == "pair_matches" {
            assert!(
                body.contains("ActiveSupport.format_db_time("),
                "{header}{body}"
            );
        }
        if method == "time_matches_value(value)" {
            assert_eq!(
                body.contains("Db.bind_text("),
                expected_binds(target),
                "{header}{body}"
            );
        }
        if method == "pair_matches" && expected_binds(target) {
            assert!(
                body.contains("Db.bind_int(stmt, 1, @key)"),
                "{header}{body}"
            );
            assert!(
                body.contains("Db.bind_text(stmt, 2, @number.to_s)"),
                "{header}{body}"
            );
            assert!(
                body.contains("Db.bind_text(stmt, 3, ActiveSupport.format_db_time(@needle))"),
                "{header}{body}"
            );
        }
    }
    let script = format!(
        r#"require_relative "boot"
require_relative "app/models/reading"
SqliteAdapter.configure("file:bind_values?mode=memory&cache=shared")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each {{ |sql| Db.exec(sql) }}
{}
Db.close
"#,
        include_str!("param_binds_values.rb")
    );
    std::fs::write(dir.join("bind_values.rb"), script).unwrap();
    println!("value gate tree: {}", dir.display());
    if target == BuildTarget::Spinel {
        success(
            Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
                .args(["bind_values.rb", "-o", "bind_values"])
                .current_dir(&*dir),
        );
        success(Command::new(dir.join("bind_values")).current_dir(&*dir));
    } else {
        let ruby = if target == BuildTarget::Jruby { "jruby" } else { "ruby" };
        success(Command::new(ruby).arg("bind_values.rb").current_dir(&*dir));
    }
}

#[test]
fn typed_values_ruby() {
    emitted("typed_values_ruby", BuildTarget::Ruby);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn typed_values_spinel() {
    emitted("typed_values_spinel", BuildTarget::Spinel);
}

#[test]
#[ignore = "requires JRuby 10+ and jdbc-sqlite3"]
fn typed_values_jruby() {
    emitted("typed_values_jruby", BuildTarget::Jruby);
}

#[test]
fn string_temporal_ivar_stays_a_string_on_strict_targets() {
    for (target, path, method, forbidden) in [
        (BuildTarget::Go, "app/v2/article.go", "SinceStringCount", "Rh_format_db_time(self.Since)"),
        (BuildTarget::Rust, "src/models/article.rs", "since_string_count", "self.since.map(crate::rh_datetime::format_db_time)"),
    ] {
        let (dir, errors) = emit_and_run::real_blog().edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def since_string_count\n    @since = \"2020-01-01 00:00:00.000000\"\n    Article.where(created_at: @since).count\n  end",
        ).emit(target);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let source = std::fs::read_to_string(dir.join(path)).unwrap();
        assert!(source.contains(method), "{target:?}: missing method");
        assert!(!source.contains(forbidden), "{target:?}: a String was passed to the native time formatter");
    }
}

#[test]
fn date_predicates_ruby() {
    if std::env::var_os("ROUNDHOUSE_BINDS_CHILD").is_none() {
        for mode in ["0", "1"] {
            success(Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "date_predicates_ruby", "--nocapture"])
                .env("ROUNDHOUSE_BINDS_CHILD", "1").env("ROUNDHOUSE_PARAM_BINDS", mode));
        }
        return;
    }
    let (dir, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :calendar_entries do |t|\n    t.date :due_on\n  end\nend\n")
        .write("sig/calendar_entry.rbs", "class CalendarEntry\n  def date_matches: (Date? value) -> Integer\n  def text_matches: (String? value) -> Integer\nend\n")
        .write("app/models/calendar_entry.rb", r#"
class CalendarEntry < ApplicationRecord
  def date_matches(value)
    @date_needle = value
    CalendarEntry.where(due_on: @date_needle).count
  end
  def text_matches(value)
    @text_needle = value
    CalendarEntry.where(due_on: @text_needle).count
  end
end
"#).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(dir.join("app/models/calendar_entry.rb")).unwrap();
    assert!(source.contains("ActiveSupport.format_db_date(@date_needle)"), "{source}");
    assert!(!source.contains("ActiveSupport.format_db_date(@text_needle)"), "{source}");
    if expected_binds(BuildTarget::Ruby) {
        assert!(source.contains("Db.bind_text_opt("), "{source}");
        assert!(source.contains(" IS NULL"), "{source}");
        assert!(source.contains(" = ?"), "{source}");
    }
    std::fs::write(dir.join("date_gate.rb"), r#"
require_relative "boot"
require_relative "app/models/calendar_entry"
SqliteAdapter.configure(":memory:")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each { |sql| Db.exec(sql) }
date = Date.new(2024, 2, 29)
row = CalendarEntry.new
row.due_on = date
row.save!
CalendarEntry.new.save!
raise "Date value" unless row.date_matches(date) == 1
raise "Date NULL" unless row.date_matches(nil) == 1
raise "String date" unless row.text_matches("2024-02-29") == 1
raise "String NULL" unless row.text_matches(nil) == 1
raise "different date" unless row.date_matches(Date.new(2024, 3, 1)) == 0
puts "typed values: native Ruby Date/String and NULL predicates passed"
Db.close
"#).unwrap();
    success(Command::new("ruby").arg("date_gate.rb").current_dir(&*dir));
}
