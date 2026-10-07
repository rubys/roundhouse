//! Generated reads must release statements before a rescued failure returns
//! to an ongoing lease, including failures before the driver's bind method.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

fn success(command: &mut Command) {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {}\n{}\n{}", output.status,
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

fn inject(path: &Path, header: &str, fault: &str) {
    let source = std::fs::read_to_string(path).unwrap();
    assert_eq!(source.matches(header).count(), 1, "{}: {header}", path.display());
    std::fs::write(path, source.replace(header,
        &format!("{header}\n    CleanupFaults.trip(\"{fault}\")"))).unwrap();
}

fn emitted(test: &str, target: BuildTarget) {
    if std::env::var_os("ROUNDHOUSE_BINDS_CHILD").is_none() {
        for mode in ["0", "1"] {
            success(Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--include-ignored", "--nocapture"])
                .env("ROUNDHOUSE_BINDS_CHILD", "1").env("ROUNDHOUSE_PARAM_BINDS", mode));
        }
        return;
    }
    let (dir, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/parents_controller.rb", "class ParentsController < ApplicationController\n  def index\n    render plain: preloaded.length.to_s\n  end\n  def preloaded\n    Parent.includes(:readings).to_a\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get '/parents', to: 'parents#index'\nend\n")
        .write("sig/reading.rbs", "class Reading\n  def time_count: (Time value) -> Integer\nend\n")
        .write("db/schema.rb", r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :parents do |t|
    t.string :name, null: false
  end
  create_table :readings do |t|
    t.integer :parent_id, null: false
    t.datetime :recorded_at, null: false
    t.string :label, null: false
  end
end
"#)
        .write("app/models/parent.rb", r#"
class Parent < ApplicationRecord
  has_many :readings
end
"#)
        .write("app/models/reading.rb", r#"
class Reading < ApplicationRecord
  belongs_to :parent
  def time_count(value)
    @needle = value
    Reading.where(recorded_at: @needle).count
  end
  def single(value)
    @id = value.to_i
    Reading.find_by(id: @id)
  end
  def many(value)
    @id = value.to_i
    Reading.where(id: @id).to_a
  end
end
"#).emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let path = dir.join("app/models/reading.rb");
    let source = std::fs::read_to_string(&path).unwrap();
    for method in ["time_count(value)", "single(value)", "many(value)", "_adapter_reload"] {
        let body = source.split_once(&format!("  def {method}\n")).unwrap().1
            .split_once("\n  end").unwrap().0;
        assert!(body.contains("\n    ensure\n      Db.finalize(stmt)"), "{method}: {body}");
    }
    let parent = std::fs::read_to_string(dir.join("app/controllers/parents_controller.rb")).unwrap();
    assert!(parent.contains("ensure\n      Db.finalize(__readings_stmt)"), "{parent}");
    inject(&path, "  def self.from_stmt(stmt)", "hydrate");
    inject(&dir.join("runtime/active_support_time_parsing.rb"), "  def self.format_db_time(value)", "serialize");
    let native = target == BuildTarget::Spinel;
    inject(&dir.join("runtime/db.rb"), if native { "  def self.bind_text(stmt, idx, value)" } else { "  def self.bind_text(handle, idx, value)" }, "bind");
    inject(&dir.join("runtime/db.rb"), if native { "  def self.step?(stmt)" } else { "  def self.step?(handle)\n    entry = handle" }, "step");
    inject(&dir.join("runtime/db.rb"), if native { "  def self.column_int(stmt, i)" } else { "  def self.column_int(handle, i)" }, "reload");
    let probe = if native {
        "class DbConn\n  def cleanup_owned_count\n    @open.length\n  end\nend\nmodule Db\n  def self.cleanup_owned_count\n    current_conn.cleanup_owned_count\n  end\nend\n"
    } else {
        "module Db\n  def self.cleanup_owned_count\n    open_statements(current_dbh).size\n  end\nend\n"
    };
    let script = format!("require_relative \"boot\"\nrequire_relative \"app/models/parent\"\nrequire_relative \"app/models/reading\"\nrequire_relative \"app/controllers/parents_controller\"\n{probe}\n{}", include_str!("param_binds_cleanup.rb"));
    std::fs::write(dir.join("cleanup_gate.rb"), script).unwrap();
    println!("cleanup tree: {} binds={}", dir.display(), std::env::var("ROUNDHOUSE_PARAM_BINDS").unwrap());
    if native {
        success(Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
            .args(["cleanup_gate.rb", "-o", "cleanup_gate"]).current_dir(&*dir));
        success(Command::new(dir.join("cleanup_gate")).current_dir(&*dir));
    } else {
        success(emit_and_run::ruby().arg("cleanup_gate.rb").current_dir(&*dir));
    }
}

#[test]
fn generated_cleanup_ruby() {
    emitted("generated_cleanup_ruby", BuildTarget::Ruby);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn generated_cleanup_spinel() {
    emitted("generated_cleanup_spinel", BuildTarget::Spinel);
}

#[test]
fn text_preprocessing_cleanup_ruby() {
    success(emit_and_run::ruby().arg("-r")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/active_record/connection_pool.rb"))
        .arg("-r")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/db_cruby.rb"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/param_binds_text_cleanup.rb")));
}
