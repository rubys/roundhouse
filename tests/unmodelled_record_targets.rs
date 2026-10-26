//! An ActiveRecord class outside app/models, such as a migration helper's
//! own query class (#1278). A target that lowers models doesn't model its
//! table, associations, scopes or callbacks, so `check` and a strict
//! build report it there (#1258). Futamura runs on Rails and the Roda
//! conversion on Sequel, which load the class themselves: they report
//! nothing. Under `--survey` the report is a recorded gap, and the output
//! is written.

use std::process::{Command, Output};

const HELPERS: &str = r#"module Tools
  module MigrationHelpers
    class CorruptionError < StandardError
    end

    class Grant < ActiveRecord::Base
      self.table_name = "information_schema.role_table_grants"

      def self.scope_to_current_user
        where("grantee = user")
      end
    end
  end
end
"#;

const REPORT: &str = "ActiveRecord model outside app/models";

struct App(std::path::PathBuf);

impl App {
    fn new(tag: &str) -> Self {
        Self::with(tag, &[])
    }

    fn with(tag: &str, extra: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("roundhouse-unmodelled-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, body) in [
            ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"members\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
            ("app/models/member.rb", "class Member < ApplicationRecord\nend\n"),
            ("lib/tools/migration_helpers.rb", HELPERS),
            ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
            (
                "config/application.rb",
                "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Sample\n  class Application < Rails::Application\n    config.load_defaults 8.0\n    config.autoload_lib(ignore: %w(assets tasks))\n  end\nend\n",
            ),
        ]
        .into_iter()
        .chain(extra.iter().copied())
        {
            let file = root.join("app_src").join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        }
        App(root)
    }

    fn run(&self, args: &[&str]) -> (Output, usize) {
        let out_dir = self.0.join(format!("out-{}", args.join("_").replace('-', "")));
        let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(args)
            .arg("-o")
            .arg(&out_dir)
            .arg(self.0.join("app_src"))
            .output()
            .expect("run roundhouse");
        let written = walk(&out_dir);
        (out, written)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn walk(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().map(|e| if e.path().is_dir() { walk(&e.path()) } else { 1 }).sum()
        })
        .unwrap_or(0)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn futamura_and_roda_report_nothing() {
    let app = App::new("rails-runtimes");
    for args in [&["--target", "futamura"][..], &["--target", "futamura", "--survey"], &["--target", "roda"]] {
        let (out, written) = app.run(args);
        assert!(out.status.success() && written > 0, "{args:?}\n{}", stderr(&out));
        assert!(!stderr(&out).contains(REPORT), "{args:?}\n{}", stderr(&out));
    }
}

#[test]
fn survey_records_it_and_writes_the_output() {
    let app = App::new("survey");
    let (out, written) = app.run(&["--target", "ruby", "--survey"]);
    assert!(out.status.success() && written > 0, "{}", stderr(&out));
    assert!(stderr(&out).contains(REPORT), "the punch list names it\n{}", stderr(&out));
}

#[test]
fn a_strict_build_of_a_lowering_target_still_reports_it() {
    let app = App::new("strict");
    let (out, _) = app.run(&["--target", "ruby"]);
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains(REPORT), "{}", stderr(&out));
}

#[test]
fn check_still_reports_it() {
    let app = App::new("check");
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(app.0.join("app_src")).output().expect("run check");
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains(REPORT) || String::from_utf8_lossy(&out.stdout).contains(REPORT), "{}", stderr(&out));
}

/// The recorded report doesn't make the file a skipped one: an unrelated
/// failure that file's classes cause elsewhere still gates the survey run,
/// rather than being attributed to the report as coverage.
#[test]
fn an_unrelated_failure_beside_it_still_gates_the_survey() {
    let helpers = HELPERS.replace(
        "    class CorruptionError < StandardError\n    end\n",
        "    class CorruptionError < StandardError\n    end\n\n    class Report\n      def total\n        1\n      end\n    end\n",
    );
    let app = App::with(
        "unrelated",
        &[
            ("lib/tools/migration_helpers.rb", helpers.as_str()),
            ("app/models/member.rb", "class Member < ApplicationRecord\n  def report_total\n    Tools::MigrationHelpers::Report.nope_here\n  end\nend\n"),
        ],
    );
    let (out, _) = app.run(&["--target", "ruby", "--survey"]);
    let err = stderr(&out);
    assert!(!out.status.success(), "the dispatch failure still fails the run\n{err}");
    assert!(err.contains("error[send_dispatch_failed]") && err.contains("nope_here"), "{err}");
    assert!(err.contains(REPORT), "and the punch list still names the class\n{err}");
}
