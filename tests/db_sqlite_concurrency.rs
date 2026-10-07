//! The CRuby Db shim's SQLite concurrency policy: the request read
//! snapshot, the in-process write permit, and checkpoints off the
//! request path (runtime/spinel/db_cruby.rb).
//!
//! Each check loads the real shim against a file database (WAL needs a
//! file) and drives it from several threads, the way Puma does. None
//! needs an emitted app: the policy lives entirely below the lowerer, in
//! `Db.exec` / `Db.prepare` / `Db.with_connection`.
//!
//! cargo test --test db_sqlite_concurrency

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::PathBuf;

fn run(name: &str, body: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-db-concurrency-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = format!(
        r#"
require {pool:?}
require {shim:?}
module ActiveRecord; class RecordNotUnique < StandardError; end; end
def check(what, ok)
  raise "FAILED: #{{what}}" unless ok
end
def count(table)
  st = Db.prepare("SELECT COUNT(*) FROM #{{table}}")
  Db.step?(st)
  n = Db.column_int(st, 0)
  Db.finalize(st)
  n
end
Db.configure({db:?}, pool_size: 4)
{body}
puts "OK"
"#,
        pool = root.join("runtime/ruby/active_record/connection_pool.rb"),
        shim = root.join("runtime/spinel/db_cruby.rb"),
        db = dir.join("test.sqlite3"),
    );
    std::fs::write(dir.join("check.rb"), script).unwrap();
    let out = emit_and_run::ruby()
        .arg("check.rb")
        .current_dir(&dir)
        .output()
        .expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim_end().ends_with("OK"),
        "{name} ({}):\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A GET's reads share one snapshot: another connection's commit is
/// invisible until the snapshot ends. A write from inside the request
/// still succeeds — without ending the snapshot first, SQLite refuses
/// the upgrade of a stale read at once with SQLITE_BUSY — and the read
/// after it sees both rows.
#[test]
fn a_get_reads_one_snapshot_and_may_still_write() {
    run(
        "snapshot",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.exec("INSERT INTO t VALUES (1)")
Db.with_connection do
  Db.read_snapshot_begin
  check("snapshot is lazy", !Db.current_dbh.transaction_active?)
  check("first read", count("t") == 1)
  check("snapshot opened by the first read", Db.current_dbh.transaction_active?)
  Thread.new { Db.with_connection { Db.exec("INSERT INTO t VALUES (2)") } }.join
  check("other connection's commit is invisible inside the snapshot", count("t") == 1)
  Db.exec("INSERT INTO t VALUES (3)")
  check("the read after a write sees every commit", count("t") == 3)
  Db.read_snapshot_end
  check("snapshot closed at the end", !Db.current_dbh.transaction_active?)
end
"#,
    );
}

/// The brackets nest (a test dispatching inside a lease, a job drained
/// inside a request) and only the outermost end closes the snapshot. A
/// request's own transaction is never joined or closed by it.
#[test]
fn snapshot_brackets_nest_and_leave_app_transactions_alone() {
    run(
        "nesting",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.with_connection do
  Db.read_snapshot_begin
  Db.read_snapshot_begin
  count("t")
  Db.read_snapshot_end
  check("inner end keeps the snapshot", Db.current_dbh.transaction_active?)
  Db.read_snapshot_end
  check("outer end closes it", !Db.current_dbh.transaction_active?)

  Db.read_snapshot_begin
  Db.exec("BEGIN")
  Db.exec("INSERT INTO t VALUES (1)")
  check("a read inside the app's transaction", count("t") == 1)
  Db.exec("COMMIT")
  check("the app's COMMIT closed its transaction", !Db.current_dbh.transaction_active?)
  Db.read_snapshot_end
end
"#,
    );
}

/// Writers in one process queue on the permit rather than racing for
/// SQLite's lock. With every connection's busy handler set to zero, a
/// second writer reaching SQLite would fail at once; none does.
#[test]
fn writers_queue_on_the_permit_not_on_sqlites_lock() {
    run(
        "permit",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
Db.instance_variable_get(:@pool).free.each { |c| c.busy_handler_timeout = 0 }
errors = Queue.new
threads = 8.times.map do |i|
  Thread.new do
    Db.with_connection do
      25.times do |j|
        begin
          if j.even?
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
          else
            Db.exec("BEGIN")
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
            sleep 0.0005
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
            Db.exec("COMMIT")
          end
        rescue => e
          errors << e
          Db.exec("ROLLBACK") rescue nil
        end
      end
    end
  end
end
threads.each(&:join)
check("no writer met SQLITE_BUSY (#{errors.size})", errors.empty?)
check("every row landed", count("t") == 8 * (13 + 12 * 2))
"#,
    );
}

/// A transaction abandoned by something `transaction`'s rescue does not
/// catch must not keep the permit (stopping every writer in the process)
/// or hand the next lease a connection mid-transaction.
#[test]
fn an_abandoned_transaction_releases_the_permit_at_lease_end() {
    run(
        "abandoned",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
begin
  Db.with_connection do
    Db.exec("BEGIN")
    Db.exec("INSERT INTO t VALUES (1)")
    raise Exception, "not a StandardError"
  end
rescue Exception
end
writer = Thread.new { Db.with_connection { Db.exec("INSERT INTO t VALUES (2)") } }
check("another writer got the permit", !writer.join(5).nil?)
check("the abandoned insert rolled back", count("t") == 1)
"#,
    );
}

/// Serving connections pin the measured page-cache / mmap knobs
/// (roundhouse#17 CRuby half). Tip used to inherit SQLite defaults
/// (`cache_size=-2000`, `mmap_size=0`) while the Spinel shim already
/// set the 64 MiB / 256 MiB budget — same working-set win, every
/// RH CRuby app, not Campfire-specific.
#[test]
fn open_connection_sets_cache_and_mmap() {
    run(
        "pragmas",
        r#"
Db.with_connection do
  c = Db.current_dbh
  check("journal_mode=WAL", c.execute("PRAGMA journal_mode")[0][0].to_s.downcase == "wal")
  check("synchronous=NORMAL", c.execute("PRAGMA synchronous")[0][0] == 1)
  check("cache_size=-65536", c.execute("PRAGMA cache_size")[0][0] == -65536)
  check("mmap_size=256MiB", c.execute("PRAGMA mmap_size")[0][0] == 268435456)
end
"#,
    );
}

/// Multi-worker checkpointing takes a non-blocking flock so only one
/// process copies the WAL (Campfire once-campfire#319's generalizable
/// bit). A second holder must fail while the first still holds it.
#[test]
fn checkpoint_flock_is_exclusive_across_processes() {
    run(
        "checkpoint_flock",
        r#"
path = Db.instance_variable_get(:@path)
lock_path = Db.checkpoint_lock_path(path)
held = Db.try_checkpoint_lock(lock_path)
check("first holder acquired", held.is_a?(File))
pid = fork do
  other = Db.try_checkpoint_lock(lock_path)
  exit(other == :busy ? 0 : 1)
end
_pid, status = Process.wait2(pid)
check("sibling saw :busy", status.exitstatus == 0)
Db.release_checkpoint_lock(held)
again = Db.try_checkpoint_lock(lock_path)
check("lock free after release", again.is_a?(File))
Db.release_checkpoint_lock(again)
# mkdir_p fails when the parent is a file — that is setup failure, not
# contention; the loop must still checkpoint (nil, not :busy).
parent = File.join(File.dirname(path), "not-a-dir")
File.write(parent, "x")
got = Db.try_checkpoint_lock(File.join(parent, "x.lock"))
check("setup failure is nil so checkpoint still runs", got.nil?)
"#,
    );
}

/// After fork, the child must close the inherited checkpoint-lock FD
/// without LOCK_UN. Otherwise a surviving worker keeps the OFD open
/// after the parent exits and its new checkpointer stays `:busy` forever
/// while `wal_autocheckpoint=0` (CodeRabbit on #548 / Puma preload).
#[test]
fn adopt_after_fork_closes_inherited_checkpoint_lock() {
    run(
        "checkpoint_flock_fork",
        r#"
path = Db.instance_variable_get(:@path)
lock_path = Db.checkpoint_lock_path(path)
held = Db.try_checkpoint_lock(lock_path)
check("parent acquired", held.is_a?(File))
check("Db retained lock file", Db.instance_variable_get(:@checkpoint_lock_file).equal?(held))
r, w = IO.pipe
pid = fork do
  w.close
  r.read(1)
  Db.adopt_after_fork
  got = Db.try_checkpoint_lock(lock_path)
  exit(got.is_a?(File) ? 0 : 1)
end
r.close
# Drop the parent's FD without unlocking — same as the parent exiting
# while the child still holds an inherited copy of the OFD.
held.close
w.write("x")
w.close
_pid, status = Process.wait2(pid)
check("child acquired after adopt closed inherited FD", status.exitstatus == 0)
"#,
    );
}

/// Once the server asks, serving connections stop checkpointing inside
/// COMMIT and a background thread copies the log into the database
/// file instead: the file grows without any request checkpointing.
#[test]
fn checkpoints_run_off_the_request_path() {
    run(
        "checkpoint",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, b BLOB)")
path = Db.instance_variable_get(:@path)
Db.checkpoint_in_background!
before = File.size(path)
Db.with_connection do
  st = Db.current_dbh.execute("PRAGMA wal_autocheckpoint")
  check("leased connection no longer autocheckpoints", st[0][0] == 0)
  200.times { Db.exec("INSERT INTO t (b) VALUES (randomblob(4096))") }
end
deadline = Time.now + 5
sleep 0.05 while File.size(path) <= before + 400_000 && Time.now < deadline
check("background checkpoint copied the log (#{before} -> #{File.size(path)})",
      File.size(path) > before + 400_000)
"#,
    );
}

/// Persistent checkpoint failures must be visible, but not every 250 ms.
/// Cap at CHECKPOINT_WARN_INTERVAL (honest visibility without a log storm).
#[test]
fn checkpoint_failure_warn_is_rate_limited() {
    run(
        "checkpoint_warn",
        r#"
require "stringio"
Db.instance_variable_set(:@checkpoint_warn_at, nil)
buf = StringIO.new
real = $stderr
$stderr = buf
begin
  Db.warn_checkpoint_failure(RuntimeError.new("first"))
  Db.warn_checkpoint_failure(RuntimeError.new("second"))
  Db.instance_variable_set(
    :@checkpoint_warn_at,
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - Db::CHECKPOINT_WARN_INTERVAL - 1
  )
  Db.warn_checkpoint_failure(RuntimeError.new("third"))
ensure
  $stderr = real
end
out = buf.string
check("first failure warned", out.include?("first"))
check("second failure suppressed inside the interval", !out.include?("second"))
check("third failure warned after the interval", out.include?("third"))
check("warn names the checkpoint", out.include?("WAL checkpoint failed"))
"#,
    );
}

/// Kill the flock holder: a surviving process's checkpointer must take
/// the lock and copy the WAL (leader election / takeover). The leader
/// only holds the flock — same shape as a wedged sibling worker — while
/// the survivor has already opted into background checkpoints and grown
/// the log with `wal_autocheckpoint=0`.
#[test]
fn checkpoint_flock_takeover_after_leader_exit() {
    run(
        "checkpoint_takeover",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, b BLOB)")
path = Db.instance_variable_get(:@path)
lock_path = Db.checkpoint_lock_path(path)
ready_r, ready_w = IO.pipe
leader = fork do
  ready_r.close
  held = Db.try_checkpoint_lock(lock_path)
  exit!(1) unless held.is_a?(File)
  ready_w.write("1")
  ready_w.close
  sleep 60
  exit!(0)
end
ready_w.close
check("leader signalled ready", ready_r.read(1) == "1")
ready_r.close

baseline = File.size(path)
Db.checkpoint_in_background!
Db.with_connection do
  st = Db.current_dbh.execute("PRAGMA wal_autocheckpoint")
  check("survivor disabled autocheckpoint", st[0][0] == 0)
  200.times { Db.exec("INSERT INTO t (b) VALUES (randomblob(4096))") }
end
# Leader still holds the flock: the survivor's loop is on :busy, so
# PASSIVE cannot run and the main db file stays near baseline while
# pages sit in the -wal file.
check("db file not yet grown under foreign flock",
      File.size(path) <= baseline + 50_000)

Process.kill("KILL", leader)
_pid, status = Process.wait2(leader)
check("leader exited", status.signaled?)

deadline = Time.now + 5
sleep 0.05 while File.size(path) <= baseline + 400_000 && Time.now < deadline
check("survivor checkpointer took over and copied (#{baseline} -> #{File.size(path)})",
      File.size(path) > baseline + 400_000)
"#,
    );
}

/// The permit wait is bounded. A transaction that starts a writer
/// thread and joins it would wait on itself forever; instead the
/// writer stops queueing after the timeout and goes to SQLite, which
/// answers exactly as it did before the permit existed: SQLITE_BUSY
/// once its busy handler gives up. A hang is worse than that error.
#[test]
fn a_writer_waiting_on_its_own_transaction_errors_instead_of_hanging() {
    run(
        "self_wait",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.instance_variable_set(:@write_permit_timeout, 0.2)
Db.instance_variable_get(:@pool).free.each { |c| c.busy_handler_timeout = 100 }
outcome = nil
Db.with_connection do
  Db.exec("BEGIN")
  Db.exec("INSERT INTO t VALUES (1)")
  child = Thread.new do
    Db.with_connection do
      begin
        Db.exec("INSERT INTO t VALUES (2)")
        outcome = :wrote
      rescue SQLite3::BusyException
        outcome = :busy
      end
    end
  end
  check("the inner writer finished rather than hanging", !child.join(5).nil?)
  Db.exec("COMMIT")
end
check("the inner writer met SQLITE_BUSY, as before the permit (#{outcome.inspect})", outcome == :busy)
check("the outer transaction committed", count("t") == 1)
check("the permit is free again", Db.acquire_permit && (Db.release_permit; true))
"#,
    );
}

/// A permit held by a thread that died (killed, not unwound) is free:
/// the Mutex it replaced was released with its owner.
#[test]
fn a_permit_held_by_a_dead_thread_is_free() {
    run(
        "dead_owner",
        r#"
Db.instance_variable_set(:@write_permit_timeout, 5)
Thread.new { Db.acquire_permit }.join
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
check("acquired", Db.acquire_permit)
Db.release_permit
check("without waiting out the timeout",
      Process.clock_gettime(Process::CLOCK_MONOTONIC) - started < 1)
"#,
    );
}

/// The same policy in the Spinel shim (runtime/spinel/db.rb), compiled
/// by spinel: tests/support/db_concurrency_spinel.rb carries the checks
/// above in the subset spinel compiles.
///
/// SPINEL=/path/to/spinel cargo test --test db_sqlite_concurrency -- --ignored
#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn spinel_shim_policy() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-db-concurrency-{}-spinel", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["db.rb", "active_support_time_parsing.rb"] {
        std::fs::copy(root.join("runtime/spinel").join(name), dir.join(name)).unwrap();
    }
    std::fs::copy(
        root.join("tests/support/db_concurrency_spinel.rb"),
        dir.join("check.rb"),
    )
    .unwrap();
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = std::process::Command::new(&compiler)
        .args(["check.rb", "-o", "check"])
        .current_dir(&dir)
        .output()
        .expect("spawn spinel");
    assert!(
        compiled.status.success(),
        "spinel failed ({}):\n{}{}",
        dir.display(),
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = std::process::Command::new(dir.join("check"))
        .arg(dir.join("test.sqlite3"))
        .current_dir(&dir)
        .output()
        .expect("run check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim_end().ends_with("OK"),
        "spinel check ({}):\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}
