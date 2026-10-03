//! Correctness gate for roundhouse#12. Both emit modes run in separate
//! processes: changing ROUNDHOUSE_PARAM_BINDS in the parallel test runner is
//! racy. No fixture generation, server, network or Docker is needed.
//!
//! cargo test --test param_binds
//! SPINEL=/path/to/spinel cargo test --test param_binds -- --ignored

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::PathBuf;
use std::process::Command;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/item.rb",
            "class Item < ApplicationRecord\n  belongs_to :parent\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/parents_controller.rb",
            "class ParentsController < ApplicationController\n  def index\n    @parents = Parent.includes(:items).to_a\n    render plain: \"ok\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get '/parents', to: 'parents#index'\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "parents", force: :cascade do |t|
    t.string "name", null: false
    t.integer "other_id", null: false
  end
  create_table "items", force: :cascade do |t|
    t.integer "parent_id", null: false
    t.string "name", null: false
  end
end
"#,
        )
        .write(
            "app/models/parent.rb",
            r#"
class Parent < ApplicationRecord
  has_many :items
  # Schema-typed ivars are the runtime values the current Arel pass binds.
  # Arbitrary method parameters deliberately fall back to Relation (IN/nil).
  def find_id(value)
    @id = value.to_i
    Item.find(@id).id
  end
  def find_by_id(value)
    @id = value.to_i
    row = Item.find_by(id: @id)
    row.nil? ? -1 : row.id
  end
  def rows(value)
    @id = value.to_i
    Item.where(id: @id).to_a
  end
  def count_id(value)
    @id = value.to_i
    Item.where(parent_id: @id).count
  end
  def exists_id(value)
    @id = value.to_i
    Item.exists?(@id)
  end
  def pair(value, parent)
    @id = value.to_i
    @other_id = parent.to_i
    Item.where(id: @id, parent_id: @other_id).count
  end
  def reload_id(value)
    @id = value.to_i
    row = Item.find(@id)
    row.name = "dirty"
    row.reload.name
  end
  def children(value)
    @id = value.to_i
    Parent.find(@id).items.to_a
  end
  def named(value)
    @name = value.to_s
    row = Item.find_by(name: @name)
    row.nil? ? -1 : row.id
  end
end
"#,
        )
}

fn success(command: &mut Command) {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    check_success(command, &output);
}

fn check_success(command: &Command, output: &std::process::Output) {
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
        for mode in ["0", "1"] {
            println!("{test}: ROUNDHOUSE_PARAM_BINDS={mode}");
            success(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", test, "--include-ignored", "--nocapture"])
                    .env("ROUNDHOUSE_BINDS_CHILD", "1")
                    .env("ROUNDHOUSE_PARAM_BINDS", mode),
            );
        }
        return;
    }
    let (dir, errors) = overlay().emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let probe = std::fs::read_to_string(dir.join("app/models/parent.rb")).unwrap();
    // Prevent a green test that silently exercises only the inline fallback.
    let binds_on = std::env::var("ROUNDHOUSE_PARAM_BINDS").unwrap() == "1";
    for method in [
        "find_by_id(value)",
        "rows(value)",
        "count_id(value)",
        "pair(value, parent)",
        "items",
    ] {
        assert_bound(&probe, method, "bind_int", binds_on);
    }
    assert_bound(&probe, "named(value)", "bind_text", binds_on);
    let item = std::fs::read_to_string(dir.join("app/models/item.rb")).unwrap();
    for method in [
        "self._adapter_find_by_id(id)",
        "self._adapter_exists_by_id?(id)",
        "_adapter_reload",
    ] {
        assert_bound(&item, method, "bind_int", binds_on);
    }
    for method in ["_adapter_insert", "_adapter_update", "_adapter_delete"] {
        let body = method_body(&item, method);
        assert!(
            !body.contains("Db.bind_"),
            "write must stay inline: {method}\n{body}"
        );
        assert!(
            body.contains("Db.exec("),
            "write must use exec: {method}\n{body}"
        );
        assert!(
            body.contains("Db.escape_"),
            "write must escape values: {method}\n{body}"
        );
    }
    let preload =
        std::fs::read_to_string(dir.join("app/controllers/parents_controller.rb")).unwrap();
    assert!(
        preload.contains("Db.escape_int_list("),
        "preload IN must stay inline:\n{preload}"
    );
    assert!(
        !preload.contains("Db.bind_"),
        "preload must not bind:\n{preload}"
    );
    assert_eq!(
        probe.contains("WHERE id = ? AND parent_id = ?"),
        binds_on,
        "{probe}"
    );
    let script = format!(
        r#"require_relative "boot"
require_relative "app/models/parent"
SqliteAdapter.configure("file:bind_gate?mode=memory&cache=shared")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each {{ |sql| Db.exec(sql) }}
{}
Db.close
"#,
        include_str!("param_binds_emit.rb")
    );
    run_script(&dir, &script, target == BuildTarget::Spinel);
    std::fs::remove_dir_all(dir.parent().unwrap()).expect("remove successful overlay");
}

fn method_body<'a>(source: &'a str, method: &str) -> &'a str {
    let header = format!("  def {method}\n");
    source
        .split_once(&header)
        .unwrap_or_else(|| panic!("missing {header}"))
        .1
        .split_once("\n  end")
        .unwrap()
        .0
}

fn assert_bound(source: &str, method: &str, bind: &str, enabled: bool) {
    let body = method_body(source, method);
    assert_eq!(
        body.contains(&format!("Db.{bind}(")),
        enabled,
        "{method}\n{body}"
    );
}

fn run_script(dir: &std::path::Path, script: &str, native: bool) {
    std::fs::write(dir.join("bind_gate.rb"), script).unwrap();
    println!("gate tree: {}", dir.display());
    if native {
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let mut command = Command::new(compiler);
        command
            .args(["bind_gate.rb", "-o", "bind_gate"])
            .current_dir(dir);
        let compiled = command.output().expect("spawn spinel");
        std::fs::write(dir.join("compile.stdout"), &compiled.stdout).unwrap();
        std::fs::write(dir.join("compile.stderr"), &compiled.stderr).unwrap();
        check_success(&command, &compiled);
        success(
            Command::new(dir.join("bind_gate"))
                .current_dir(dir)
                // The runtime's environment override wins over pool_size:
                // a caller's value of 1 would deadlock our four-lease barrier.
                .env("DATABASE_POOL_SIZE", "4"),
        );
    } else {
        success(emit_and_run::ruby().arg("bind_gate.rb").current_dir(dir));
    }
}

fn runtime(native: bool) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!(
        "roundhouse-bind-runtime-{}-{native}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let prelude = if native {
        for name in ["db.rb", "active_support_time_parsing.rb"] {
            std::fs::copy(root.join("runtime/spinel").join(name), dir.join(name)).unwrap();
        }
        "require_relative \"db\"\n".to_string()
    } else {
        format!(
            "require {:?}\nrequire {:?}\n",
            root.join("runtime/ruby/active_record/connection_pool.rb"),
            root.join("runtime/spinel/db_cruby.rb")
        )
    };
    let lifecycle = if native {
        r#"
# Spinel promises clear_bindings at release; CRuby instead promises complete
# rebinding and deliberately retains old values. Probe the native contract
# directly, since a generated reader overwrites every slot and cannot see it.
stmt = Db.prepare("SELECT COALESCE(?, -99)")
Db.bind_int(stmt, 1, 73)
raise "missing clear seed" if !Db.step?(stmt)
expect_int("clear seed", 73, Db.column_int(stmt, 0))
Db.finalize(stmt)
stmt = Db.prepare("SELECT COALESCE(?, -99)")
raise "missing clear probe" if !Db.step?(stmt)
expect_int("finalize clears bindings", -99, Db.column_int(stmt, 0))
Db.finalize(stmt)
puts "runtime: finalize clears bindings passed"
"#
    } else {
        r#"
# Finalize must release a partially consumed reader's lock before this
# connection is reused. Holding its lease forces the writer onto another
# connection; a prepare-time reset cannot mask a missing finalize reset.
Db.exec("CREATE TABLE bind_lock_rows (id INTEGER PRIMARY KEY)")
Db.exec("INSERT INTO bind_lock_rows VALUES (1), (2)")
Db.with_connection do
  stmt = Db.prepare("SELECT id FROM bind_lock_rows WHERE id >= ? ORDER BY id")
  Db.bind_int(stmt, 1, 1)
  raise "missing lock probe" unless Db.step?(stmt)
  expect_int("partial reader", 1, Db.column_int(stmt, 0))
  Db.finalize(stmt)
  writer = Thread.new do
    Db.with_connection do
      Db.exec("UPDATE bind_lock_rows SET id = 3 WHERE id = 2")
      Db.changes
    end
  end
  expect_int("finalize releases read lock", 1, writer.value)
end
puts "runtime: CRuby finalize releases partial reader before another connection writes"

# A reader that raises before finalize leaves its cached statement stepped.
# Rebind on the same connection before cleaning up the abandoned handle:
# finalize's reset must not mask removal of the prepare-time reset.
Db.with_connection do
  interrupted = nil
  begin
    begin
      interrupted = Db.prepare("SELECT id, label FROM bind_rows WHERE id = ?")
      Db.bind_int(interrupted, 1, 1)
      raise "missing interrupted reader" unless Db.step?(interrupted)
      expect_int("interrupted reader", 1, Db.column_int(interrupted, 0))
      raise "deliberately interrupted before finalize"
    rescue RuntimeError => error
      raise unless error.message == "deliberately interrupted before finalize"
    end
    read_bound_id(19)
  ensure
    Db.finalize(interrupted)
  end
end
puts "runtime: CRuby prepare recovers an interrupted reader with a different id"
"#
    };
    // Run CRuby's lifecycle probes before any other reader can hold a lock.
    // Check Spinel cleanup before NUL, so missing clear has its own failure.
    let body = include_str!("param_binds_runtime.rb");
    let marker = if native {
        "# Observe bytes as a BLOB"
    } else {
        "# A live outer cursor"
    };
    assert_eq!(
        body.matches(marker).count(),
        1,
        "missing or ambiguous lifecycle probe marker"
    );
    let body = body.replace(marker, &format!("{lifecycle}\n{marker}"));
    let script = format!(
        "{prelude}\nDb.configure(\"file:bind_runtime?mode=memory&cache=shared\", pool_size: 4)\n{body}\nDb.close\n"
    );
    run_script(&dir, &script, native);
    std::fs::remove_dir_all(dir).expect("remove successful runtime probe");
}

#[test]
fn varying_binds_ruby() {
    emitted("varying_binds_ruby", BuildTarget::Ruby);
}

#[test]
fn bind_runtime_ruby() {
    runtime(false);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn varying_binds_spinel() {
    emitted("varying_binds_spinel", BuildTarget::Spinel);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn bind_runtime_spinel() {
    runtime(true);
}
