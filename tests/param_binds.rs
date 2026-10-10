//! Correctness gate for roundhouse#12. The default and both override modes run in separate
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

// These probes cover supported runtimes. Pin their default
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
    t.integer "number"
    t.string "optional_name"
    t.boolean "flag"
  end
  create_table "items", force: :cascade do |t|
    t.integer "parent_id", null: false
    t.string "name", null: false
    t.integer "number"
    t.string "optional_name"
    t.boolean "flag"
    t.boolean "required_flag", null: false
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
  def nullable_pair(id, number, name, flag, parent)
    @id = id.to_i
    @number = number.nil? ? nil : number.to_i
    @optional_name = name.nil? ? nil : name.to_s
    @flag = flag.nil? ? nil : flag == true
    @other_id = parent.to_i
    Item.where(id: @id, number: @number, optional_name: @optional_name, flag: @flag, parent_id: @other_id).count
  end
  def nullable_key(number)
    @number = number.nil? ? nil : number.to_i
    Item.where(id: @number).count
  end
  def not_null_number(number)
    @number = number.nil? ? nil : number.to_i
    Item.where(parent_id: @number).count
  end
  def not_null_name(name)
    @optional_name = name.nil? ? nil : name.to_s
    Item.where(name: @optional_name).count
  end
  def not_null_flag(flag)
    @flag = flag.nil? ? nil : flag == true
    Item.where(required_flag: @flag).count
  end
  def nil_key_exists
    Item.exists?(nil)
  end
  def nil_key_find
    Item.find(nil)
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
    let probe = std::fs::read_to_string(dir.join("app/models/parent.rb")).unwrap();
    // Prevent a green test that silently exercises only the inline fallback.
    let binds_on = expected_binds(target);
    for method in [
        "find_by_id(value)",
        "rows(value)",
        "count_id(value)",
        "pair(value, parent)",
        "nullable_pair(id, number, name, flag, parent)",
        "items",
    ] {
        assert_bound(&probe, method, "bind_int", binds_on);
    }
    assert_bound(&probe, "named(value)", "bind_text", binds_on);
    let nullable = probe.split("  def nullable_pair(").nth(1).unwrap();
    let nullable = nullable.split("\n  end").next().unwrap();
    for (column, escape) in [
        ("number", "escape_int_opt"),
        ("optional_name", "escape_string_opt"),
        ("flag", "escape_bool_opt"),
    ] {
        assert!(nullable.contains(&format!("{column} IS NULL")), "{nullable}");
        assert!(!nullable.contains(&format!("{column} IS ?")), "{nullable}");
        if !binds_on {
            assert!(nullable.contains(&format!("Db.{escape}(")), "{nullable}");
        }
    }
    if binds_on {
        assert_eq!(nullable.matches("Db.bind_").count(), 5, "{nullable}");
        assert!(nullable.contains("Db.bind_int("), "{nullable}");
        assert!(nullable.contains("Db.bind_int_opt("), "{nullable}");
        assert!(nullable.contains("Db.bind_text_opt("), "{nullable}");
        assert!(nullable.contains("Db.bind_bool_opt("), "{nullable}");
        let key = probe.split("  def nullable_key(").nth(1).unwrap().split("\n  end").next().unwrap();
        assert!(key.contains("WHERE id = ?"), "{key}");
        assert!(key.contains("Db.bind_int_opt(stmt, 1, @number)"), "{key}");
    }
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
    // JRuby's real JDBC execution is covered by its container contract and
    // compare lane. This host-independent probe pins its compiler policy.
    if target != BuildTarget::Jruby {
        run_script(&dir, &script, target == BuildTarget::Spinel);
    }
    std::fs::remove_dir_all(dir.parent().unwrap()).expect("remove successful overlay");
}

#[test]
fn emitted_reads_jruby_policy() {
    emitted("emitted_reads_jruby_policy", BuildTarget::Jruby);
}

#[test]
fn direct_ruby_and_spinel_emit_use_their_defaults() {
    let mut app = roundhouse::ingest::ingest_app(std::path::Path::new("fixtures/tiny-blog"))
        .expect("tiny-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    for (label, files, binds) in [
        ("Ruby", roundhouse::emit::ruby::emit_lowered_models(&app), false),
        ("Spinel", roundhouse::emit::ruby::emit_spinel(&app), true),
    ] {
        let models: Vec<_> = files.iter()
            .filter(|f| f.content.contains("  def self._adapter_find_by_id(id)\n"))
            .collect();
        assert!(!models.is_empty(), "{label}: no synthesized reads");
        for model in models {
            assert_bound(&model.content, "self._adapter_find_by_id(id)", "bind_int", binds);
        }
    }
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

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
    let _cleanup = ScratchDir(dir.clone());
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
    // The shims raise ActiveRecord::RecordNotUnique, which an app gets
    // from runtime/ruby/active_record/errors.rb; stub it as the other
    // shim harnesses do.
    let prelude = format!(
        "{prelude}module ActiveRecord\n  class RecordNotUnique < StandardError\n  end\nend\n"
    );
    let clear = r#"
# Both shims clear bindings on release. Probe a missing bind on idle reuse
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
"#;
    let lifecycle = if native {
        ""
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

# An interrupted reader still owns its cached statement. A nested read uses
# a transient sibling; it must neither reset nor rebind the live owner.
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
puts "runtime: CRuby interrupted reader keeps ownership across a different id"
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
    let nil_contract = include_str!("param_binds_nil.rb");
    let body = body.replace(marker, &format!("{nil_contract}\n{lifecycle}\n{marker}"));
    let body = body.replace(
        "# Observe bytes as a BLOB",
        &format!("{clear}\n# Observe bytes as a BLOB"),
    );
    let cache_cases = include_str!("../runtime/spinel/test/statement_cache_cases.rb");
    let ownership = if native {
        include_str!("param_binds_spinel_cache.rb")
    } else {
        include_str!("param_binds_cruby_cache.rb")
    };
    // Spinel's existing inline writer cannot put NUL in SQL text. Its
    // bind-byte contract above still covers NUL; the gem/JDBC writers use
    // BLOB literals, so the gem path also checks writer/reader parity.
    let nul_writer = if native {
        ""
    } else {
        r#"
inline_bound_string("NUL UTF-8", "a\0b")
inline_bound_string("NUL binary", "a\0b".b)
inline_bound_string("invalid UTF-8 tag", "\xFFa".force_encoding(Encoding::UTF_8))
inline_bound_string("gem BLOB wrapper", SQLite3::Blob.new("plain-ascii"))
puts "runtime: NUL inline writes and bound reads agree"
"#
    };
    let script = format!(
        "{prelude}\nENV[\"DATABASE_POOL_SIZE\"] = \"1\"\n\
         Db.configure(\"file:cache_cases?mode=memory&cache=shared\", pool_size: 1)\n\
         {cache_cases}\nStatementCacheTest.new.run\n\
         puts \"runtime: 12 statement cache ownership and error tests passed\"\nDb.close\n\
         ENV[\"DATABASE_POOL_SIZE\"] = \"4\"\n\
         Db.configure(\"file:bind_runtime?mode=memory&cache=shared\", pool_size: 4)\n{body}\n{nul_writer}\n{ownership}\nDb.close\n"
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

// A String-only override exposes the inherited Integer adapter seed that
// SQLite's escape helpers otherwise hide. Compile the actual emitted RBS;
// an unseeded native compile does not exercise this boundary.
fn string_key_adapter(test: &str, target: BuildTarget) {
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
    let (dir, errors) = emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  self.primary_key = \"identifier\"\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", primary_key: \"identifier\", id: :string do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let _cleanup = ScratchDir(dir.parent().unwrap().to_path_buf());
    let script = r#"require_relative "boot"
class Widget
  def self._adapter_find_by_id(id)
    record = Widget.new
    record.name = id.upcase
    record
  end
  def self._adapter_exists_by_id?(id)
    id.upcase == "LITERAL_KEY"
  end
end
raise "String adapter input" unless Widget.find("literal_key").name == "LITERAL_KEY"
raise "empty String key" unless Widget.find("").name == ""
raise "zero String key" unless Widget.find("0").name == "0"
raise "String exists adapter" unless Widget.exists?("literal_key")
raise "missing String exists adapter" if Widget.exists?("other")
raise "nil exists guard" if Widget.exists?(nil)
begin
  Widget.find(nil)
  raise "nil reached adapter"
rescue ActiveRecord::RecordNotFound
end
puts "PASS seeded String-only find/exists adapter boundary and nil guards"
"#;
    if target == BuildTarget::Spinel {
        std::fs::write(dir.join("strict_adapter.rb"), script).unwrap();
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let mut command = Command::new(compiler);
        command
            .args(["--rbs", ".", "strict_adapter.rb", "-o", "strict_adapter"])
            .current_dir(&dir);
        let output = command.output().expect("compile seeded String adapter");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("type seeds are unavailable"),
            "RBS extractor is required: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        check_success(&command, &output);
        success(Command::new(dir.join("strict_adapter")).current_dir(&dir));
    } else {
        run_script(&dir, script, false);
    }
}

#[test]
fn string_key_adapter_ruby() {
    string_key_adapter("string_key_adapter_ruby", BuildTarget::Ruby);
}

#[test]
#[ignore = "requires Spinel and its RBS extractor"]
fn string_key_adapter_spinel() {
    string_key_adapter("string_key_adapter_spinel", BuildTarget::Spinel);
}

fn nullable_associations(test: &str, target: BuildTarget) {
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
    let app = emit_and_run::empty_app()
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
            "app/models/account.rb",
            "class Account < ApplicationRecord\n  has_many :links\n  has_many :taggings, as: :taggable\nend\n",
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :links\nend\n",
        )
        .write(
            "app/models/link.rb",
            "class Link < ApplicationRecord\n  belongs_to :account, optional: true\n  belongs_to :article, optional: true\nend\n",
        )
        .write(
            "app/models/tagging.rb",
            "class Tagging < ApplicationRecord\n  belongs_to :taggable, polymorphic: true, optional: true\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "accounts", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "articles", id: :uuid, force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "links", force: :cascade do |t|
    t.integer "account_id"
    t.uuid "article_id"
  end
  create_table "taggings", force: :cascade do |t|
    t.integer "taggable_id"
    t.string "taggable_type"
  end
end
"#,
        );
    let (dir, errors) = app.emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let binds_on = expected_binds(target);
    let link = std::fs::read_to_string(dir.join("app/models/link.rb")).unwrap();
    assert_bound(&link, "account", "bind_int", binds_on);
    assert_bound(&link, "article", "bind_text", binds_on);
    let tagging = std::fs::read_to_string(dir.join("app/models/tagging.rb")).unwrap();
    assert_bound(&tagging, "taggable", "bind_int", binds_on);
    // Spinel currently boxes column_int_opt's nil sentinel as an Integer
    // when passing it to a generated setter. That separate hydration gap
    // makes .nil? false before this reader runs. Exercise native integer
    // nil inputs through the post-emission assignments in the shared
    // script, and retain native SQL-NULL hydration coverage for UUIDs.
    let integer_hydration = if target == BuildTarget::Spinel {
        ""
    } else {
        r#"
raise "nil integer FK from SQL resolved an association" unless from_sql.account.nil?
raise "nil polymorphic FK from SQL resolved an association" unless Tagging.find(1).taggable.nil?
raise "nil polymorphic type from SQL resolved an association" unless Tagging.find(3).taggable.nil?
puts "emit: SQL-NULL integer and polymorphic association readers passed"
"#
    };
    let script = format!(
        r#"require_relative "boot"
require_relative "app/models/account"
require_relative "app/models/article"
require_relative "app/models/link"
require_relative "app/models/tagging"
SqliteAdapter.configure("file:nil_associations?mode=memory&cache=shared")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each {{ |sql| Db.exec(sql) }}
{}
{}
from_sql = Link.find(1)
raise "nil UUID FK from SQL resolved an association" unless from_sql.article.nil?
puts "emit: SQL-NULL UUID association reader passed"
{integer_hydration}
Db.close
"#,
        cache_probe(target == BuildTarget::Spinel),
        include_str!("param_binds_associations.rb")
    );
    run_script(&dir, &script, target == BuildTarget::Spinel);
}

#[test]
fn nullable_associations_ruby() {
    nullable_associations("nullable_associations_ruby", BuildTarget::Ruby);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn nullable_associations_spinel() {
    nullable_associations("nullable_associations_spinel", BuildTarget::Spinel);
}

fn cache_probe(native: bool) -> &'static str {
    if native {
        r#"
module SQL
  ffi_func :sqlite3_next_stmt, [:ptr, :ptr], :ptr
end
class DbConn
  def gate_cache_size
    @entries.length
  end
  def gate_live_statements
    n = 0
    ptr = SQL.sqlite3_next_stmt(dbh, nil)
    while !ptr.nil?
      n += 1
      ptr = SQL.sqlite3_next_stmt(dbh, ptr)
    end
    n
  end
end
module Db
  def self.gate_cache_size
    current_conn.gate_cache_size
  end
  def self.gate_live_statements
    current_conn.gate_live_statements
  end
  def self.gate_released(stmt)
    nil
  end
end
"#
    } else {
        r#"
module Db
  def self.gate_cache_size
    (current_dbh.instance_variable_get(:@rh_stmt_cache) || {}).size
  end
  def self.gate_live_statements
    0
  end
  def self.gate_released(stmt)
    raise "transient statement was not closed" if stmt[:stmt] && !stmt[:stmt].closed?
  end
end
"#
    }
}

fn raw_where_substitution(target: BuildTarget) {
    let (dir, errors) = overlay().emit(target);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let script = format!(
        r#"require_relative "boot"
require_relative "app/models/item"
SqliteAdapter.configure("file:raw_where_gate?mode=memory&cache=shared")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each {{ |sql| Db.exec(sql) }}
{}
Db.close
"#,
        include_str!("param_binds_raw_where.rb")
    );
    run_script(&dir, &script, target == BuildTarget::Spinel);
    std::fs::remove_dir_all(dir.parent().unwrap()).expect("remove successful overlay");
}

#[test]
fn raw_where_substitution_ruby() {
    raw_where_substitution(BuildTarget::Ruby);
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn raw_where_substitution_spinel() {
    raw_where_substitution(BuildTarget::Spinel);
}

#[test]
fn typescript_profiles_keep_inline_reads() {
    use roundhouse::profile::DeploymentProfile;
    let mut app = roundhouse::ingest::ingest_app(std::path::Path::new("fixtures/tiny-blog"))
        .expect("tiny-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    for profile in [
        DeploymentProfile::node_sync(),
        DeploymentProfile::node_async(),
        DeploymentProfile::worker(),
    ] {
        let files = roundhouse::emit::typescript::emit_with_profile(&app, &profile);
        let models: Vec<_> = files.iter()
            .filter(|f| f.content.contains("_adapter_find_by_id"))
            .collect();
        assert!(!models.is_empty(), "{} must exercise synthesized reads", profile.name);
        assert!(models.iter().any(|f| f.content.contains("Db.escape_int(")), "{}", profile.name);
        assert!(models.iter().all(|f| !f.content.contains("Db.bind_")), "{}", profile.name);
    }
}
