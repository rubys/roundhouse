//! `validates :attr, inclusion: { in: <enum plural>.keys }` — an enum the
//! class body declared, whose `keys` are its labels — folds to that list,
//! as an `in:` list constant does. Before, the call was "inclusion: in: not
//! supported". Only an enum whose labels are the Strings it stores folds
//! (`{ password: "password" }`): the lowered check reads the stored value,
//! and an integer enum stays a reported gap. Answers are activerecord
//! 8.1.4's (`rails_answers_the_table`).

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::process::Command;

const MODEL: &str = r#"class Login < ApplicationRecord
  enum :method_name, { password: "password", otp: "otp" }
  enum :channel, { web: "web", app: "app" }

  validates :method_name, inclusion: { in: method_names.keys }
  validates :channel, inclusion: { in: self.channels.keys }
end
"#;

/// `(method_name, channel)` → error messages.
const CASES: &[(&str, &str, &str)] = &[
    ("nil", "nil", r#"["Method name is not included in the list", "Channel is not included in the list"]"#),
    (r#""password""#, r#""web""#, "[]"),
    (":otp", r#""app""#, "[]"),
    (r#""otp""#, "nil", r#"["Channel is not included in the list"]"#),
];

fn assertions() -> String {
    CASES
        .iter()
        .map(|(m, c, want)| {
            format!(
                "l = Login.new(method_name: {m}, channel: {c})\nl.valid?\ngot = messages(l).inspect\nraise %q({m}/{c}: ) + got unless got == {want:?}\n"
            )
        })
        .collect()
}

fn app(model: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"logins\", force: :cascade do |t|\n    t.string \"method_name\"\n    t.string \"channel\"\n    t.integer \"status\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/login.rb", model)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn the_ruby_target_checks_an_enums_keys_as_rails_does() {
    app(MODEL)
        .run_ruby(&format!("def messages(r)\n  r.errors\nend\n{}puts \"PASS\"\n", assertions()))
        .assert_passes();
}

/// An integer enum stores 0/1 where its labels are Strings: the list does
/// not fold, and the call stays a reported gap.
#[test]
fn an_integer_enums_keys_stay_a_gap() {
    let model = "class Login < ApplicationRecord\n  enum :status, { draft: 0, live: 1 }\n\n  validates :status, inclusion: { in: statuses.keys }\nend\n";
    let validations = roundhouse::ingest::ingest_app_from_tree(
        [
            ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"logins\", force: :cascade do |t|\n    t.integer \"status\"\n  end\nend\n"),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
            ("app/models/login.rb", model),
            ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ]
        .into_iter()
        .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
        .collect(),
    )
    .map(|app| format!("{:?}", app.models[0].validations().collect::<Vec<_>>()))
    .expect("ingest");
    assert!(!validations.contains("\"draft\""), "an integer enum's labels must not fold: {validations}");
}

/// The table itself, against activerecord 8.1.4 on sqlite3.
#[test]
fn rails_answers_the_table() {
    let probe = r#"gem "activerecord", "8.1.4"; require "active_record"; require "sqlite3""#;
    if !Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord 8.1.4 and sqlite3 not available");
        return;
    }
    let script = format!(
        r##"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:logins) {{ |t| t.string :method_name; t.string :channel; t.integer :status }}
end
class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
{MODEL}
def messages(r)
  r.errors.full_messages
end
{asserts}puts "Rails agrees"
"##,
        asserts = assertions(),
    );
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "activerecord 8.1.4 disagrees with the table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}
