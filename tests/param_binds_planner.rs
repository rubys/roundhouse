//! Runtime nullable shapes must preserve SQLite's partial-index eligibility
//! and outer-join strength reduction. Plans are captured from emitted reads,
//! not independently reconstructed predicates. Each flag gets a subprocess.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::process::Command;

#[test]
fn nullable_read_plans_ruby() {
    if std::env::var_os("ROUNDHOUSE_BINDS_CHILD").is_none() {
        for mode in ["0", "1"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "nullable_read_plans_ruby", "--nocapture"])
                .env("ROUNDHOUSE_BINDS_CHILD", "1")
                .env("ROUNDHOUSE_PARAM_BINDS", mode)
                .output().unwrap();
            assert!(output.status.success(), "binds={mode}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        return;
    }
    let (dir, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :indexed_rows do |t|
    t.integer :a
  end
  create_table :paired_rows do |t|
    t.integer :a
    t.integer :b
  end
  create_table :host_rows do |t|
  end
  create_table :joined_rows do |t|
    t.integer :a
  end
  create_table :wide_rows do |t|
    t.integer :a
    t.integer :b
    t.integer :c
    t.integer :d
    t.integer :e
    t.integer :f
    t.integer :g
    t.integer :h
  end
end
"#)
        .write("app/models/indexed_row.rb", r#"
class IndexedRow < ApplicationRecord
  def matching(value)
    @a = value.nil? ? nil : value.to_i
    IndexedRow.where(a: @a).count
  end
end
"#)
        .write("app/models/paired_row.rb", r#"
class PairedRow < ApplicationRecord
  def matching(first, second)
    @a = first.nil? ? nil : first.to_i
    @b = second.nil? ? nil : second.to_i
    PairedRow.where(a: @a, b: @b).count
  end
end
"#)
        .write("app/models/joined_row.rb", r#"
class JoinedRow < ApplicationRecord
  def matching(value)
    @a = value.nil? ? nil : value.to_i
    JoinedRow.where(a: @a).count
  end
end
"#)
        .write("app/models/wide_row.rb", r#"
class WideRow < ApplicationRecord
  def matching(a, b, c, d, e, f, g, h)
    @a = a.nil? ? nil : a.to_i
    @b = b.nil? ? nil : b.to_i
    @c = c.nil? ? nil : c.to_i
    @d = d.nil? ? nil : d.to_i
    @e = e.nil? ? nil : e.to_i
    @f = f.nil? ? nil : f.to_i
    @g = g.nil? ? nil : g.to_i
    @h = h.nil? ? nil : h.to_i
    WideRow.where(a: @a, b: @b, c: @c, d: @d, e: @e, f: @f, g: @g, h: @h).count
  end
end
"#)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let bound = std::env::var("ROUNDHOUSE_PARAM_BINDS").unwrap() == "1";
    for model in ["indexed_row", "paired_row", "joined_row", "wide_row"] {
        let source = std::fs::read_to_string(dir.join(format!("app/models/{model}.rb"))).unwrap();
        let method = source.split_once("  def matching(").unwrap().1.split_once("\n  end").unwrap().0;
        assert!(!method.contains(".where("), "read was not lowered: {method}");
        assert_eq!(method.contains("Db.bind_"), bound, "{method}");
        if model == "wide_row" && bound {
            assert!(method.contains("Db.prepare_uncached("), "eight nullable predicates must bypass the 128-shape cache: {method}");
        } else {
            assert!(method.contains("Db.prepare("), "{method}");
        }
    }
    std::fs::write(dir.join("planner_gate.rb"), include_str!("param_binds_planner.rb")).unwrap();
    let output = emit_and_run::ruby().arg("planner_gate.rb").current_dir(&*dir)
        .env("PLANNER_BINDS", if bound { "1" } else { "0" }).output().unwrap();
    assert!(output.status.success(), "{}\n{}\n{}", dir.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}
