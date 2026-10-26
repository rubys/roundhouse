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
require {errors:?}
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
        errors = root.join("runtime/ruby/active_record/errors.rb"),
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

/// Transaction control and the observer's `PRAGMA data_version` are
/// compiled once per connection, not on every call (the gem's `execute`
/// and `get_first_value` prepare and close each time): on campfire's
/// users profile they were about 10 of the ~14 statements a request
/// prepared. Reuse must not change what they do: a failing COMMIT still
/// raises and the statement runs again after it, `data_version` still
/// moves when another connection commits (the reused statement holds no
/// snapshot), and `close` finalizes them.
#[test]
fn fixed_statements_compile_once_per_connection() {
    run(
        "fixed",
        r#"
module PrepareCount
  COUNT = Hash.new(0)
  def prepare(sql, *args, &block)
    PrepareCount::COUNT[sql] += 1
    super
  end
end
SQLite3::Database.prepend(PrepareCount)

Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.with_connection do
  50.times do |i|
    Db.exec("BEGIN")
    Db.exec("INSERT INTO t VALUES (#{i})")
    Db.exec("COMMIT")
  end
end
check("every row committed", count("t") == 50)
check("BEGIN IMMEDIATE compiled once, not per transaction", PrepareCount::COUNT["BEGIN IMMEDIATE"] == 1)
check("COMMIT compiled once, not per transaction", PrepareCount::COUNT["COMMIT"] == 1)

conn = SQLite3::Database.new(Db.database_path)
failed = false
begin
  conn.execute("COMMIT")
rescue SQLite3::Exception
  failed = true
end
check("a COMMIT outside a transaction still raises", failed)
conn.execute("BEGIN")
conn.execute("INSERT INTO t VALUES (1000)")
conn.execute("ROLLBACK")
check("the statements run again after one raised", !conn.transaction_active?)
check("the rollback rolled back", count("t") == 50)
conn.execute("BEGIN")
conn.execute("INSERT INTO t VALUES (1001)")
conn.execute("COMMIT")
check("and commit", count("t") == 51)

observer = SQLite3::Database.new(Db.database_path)
v1 = observer.get_first_value("PRAGMA data_version")
conn.execute("INSERT INTO t VALUES (1002)")
v2 = observer.get_first_value("PRAGMA data_version")
v3 = observer.get_first_value("PRAGMA data_version")
check("data_version moves when another connection commits", v2 != v1)
check("and holds when nothing does", v3 == v2)
check("data_version compiled once", PrepareCount::COUNT["PRAGMA data_version"] == 1)
check("a block or binds take the gem's own path", conn.execute("SELECT 1", []).first == [1])

observer.close
conn.close
check("close closes", conn.closed? && observer.closed?)
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

/// Writers in separate PROCESSES (Puma's forked workers) queue on the
/// permit's flock rather than meeting at SQLite's lock. With every
/// connection's busy handler at zero, a writer reaching SQLite while
/// another process holds the lock would fail at once; none does.
#[test]
fn writers_in_separate_processes_queue_on_the_writer_flock() {
    run(
        "writer_flock",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
pids = 4.times.map do |i|
  fork do
    Db.adopt_after_fork
    Db.instance_variable_get(:@pool).free.each { |c| c.busy_handler_timeout = 0 }
    failed = 0
    Db.with_connection do
      40.times do
        begin
          Db.exec("BEGIN")
          Db.exec("INSERT INTO t (n) VALUES (#{i})")
          sleep 0.0005
          Db.exec("INSERT INTO t (n) VALUES (#{i})")
          Db.exec("COMMIT")
        rescue => e
          failed += 1
          Db.exec("ROLLBACK") rescue nil
        end
      end
    end
    exit!(failed == 0 ? 0 : 1)
  end
end
statuses = pids.map { |pid| Process.wait2(pid)[1].exitstatus }
check("no process met SQLITE_BUSY (#{statuses.inspect})", statuses.all?(0))
check("every row landed", count("t") == 4 * 40 * 2)
"#,
    );
}

/// The writer flock is held per open file, so a child that kept its
/// parent's descriptor would count as the same holder and walk straight
/// in. A forked child opens its own: while the parent holds the lock the
/// child's wait ends at the permit's deadline without it, and once the
/// parent lets go the child takes it. The parent opens the lock file
/// before the fork, as a server that wrote while booting does, and takes
/// the lock after it, as a worker does.
#[test]
fn a_forked_child_waits_for_its_parents_writer_flock() {
    run(
        "writer_flock_fork",
        r#"
check("the parent opens the lock file", Db.acquire_permit && Db.instance_variable_get(:@writer_lock_held))
Db.release_permit
go_r, go_w = IO.pipe
report_r, report_w = IO.pipe
freed_r, freed_w = IO.pipe
pid = fork do
  go_w.close; report_r.close; freed_w.close
  go_r.gets
  Db.write_permit_timeout = 0.2
  Db.acquire_permit
  waited_out = !Db.instance_variable_get(:@writer_lock_held)
  Db.release_permit
  report_w.puts(waited_out ? "waited" : "walked in")
  report_w.close
  freed_r.gets
  Db.write_permit_timeout = 5.0
  Db.acquire_permit
  took = Db.instance_variable_get(:@writer_lock_held)
  Db.release_permit
  exit!(took ? 0 : 1)
end
go_r.close; report_w.close; freed_r.close
check("parent takes the lock", Db.acquire_permit && Db.instance_variable_get(:@writer_lock_held))
go_w.puts("go")
check("the child's wait ended without the parent's lock", report_r.gets.to_s.strip == "waited")
Db.release_permit
check("the parent let go", !Db.instance_variable_get(:@writer_lock_held))
freed_w.puts("free")
_pid, status = Process.wait2(pid)
check("the child took the lock once it was free", status.exitstatus == 0)
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

/// A server that checkpoints in a process of its own (falcon.rb): after
/// `checkpoint_in_own_process!` a serving lease turns automatic
/// checkpoints off and starts no thread, and `run_checkpointer` in
/// another process copies the WAL into the database file.
#[test]
fn checkpoints_run_in_a_process_of_their_own() {
    run(
        "checkpoint_own_process",
        r#"
path = Db.instance_variable_get(:@path)
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, body TEXT)")
Db.checkpoint_in_own_process!
threads = Thread.list.size
Db.with_connection do
  c = Db.current_dbh
  check("serving connection leaves checkpoints to the checkpointer", c.execute("PRAGMA wal_autocheckpoint")[0][0] == 0)
end
check("no checkpoint thread in a serving process", Thread.list.size == threads)
check("no checkpointer pid in a serving process", Db.instance_variable_get(:@checkpointer_pid).nil?)
200.times { |i| Db.exec("INSERT INTO t (body) VALUES ('row #{i}')") }

# The database file alone, without its WAL: what a checkpoint has copied.
def rows_in_main_file(path)
  copy = path + ".copy"
  File.binwrite(copy, File.binread(path))
  db = SQLite3::Database.new(copy)
  n = (db.execute("SELECT COUNT(*) FROM t")[0][0] rescue 0)
  db.close
  File.delete(copy)
  n
end
check("rows still only in the WAL", rows_in_main_file(path) < 200)

pid = fork do
  Db.configure(path, pool_size: 1)
  Db.run_checkpointer
end
deadline = Time.now + 10
sleep 0.1 until rows_in_main_file(path) == 200 || Time.now > deadline
Process.kill("KILL", pid)
Process.wait(pid)
check("the checkpointer process copied the WAL", rows_in_main_file(path) == 200)
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

/// `before_worker_boot` calls `Db.configure`, which sets `@owner_pid` to
/// the child and skips `adopt_after_fork`. Configure must still close an
/// inherited checkpoint-lock FD without LOCK_UN and clear the parent's
/// checkpointer pid.
#[test]
fn configure_closes_inherited_checkpoint_lock_after_fork() {
    run(
        "checkpoint_flock_configure",
        r#"
path = Db.instance_variable_get(:@path)
lock_path = Db.checkpoint_lock_path(path)
held = Db.try_checkpoint_lock(lock_path)
check("parent acquired", held.is_a?(File))
Db.instance_variable_set(:@checkpointer_pid, Process.pid)
r, w = IO.pipe
pid = fork do
  w.close
  r.read(1)
  Db.configure(path, pool_size: 2)
  check("checkpointer pid cleared", Db.instance_variable_get(:@checkpointer_pid).nil?)
  got = Db.try_checkpoint_lock(lock_path)
  exit(got.is_a?(File) ? 0 : 1)
end
r.close
held.close
w.write("x")
w.close
_pid, status = Process.wait2(pid)
check("configure closed inherited FD so child acquired", status.exitstatus == 0)
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

/// Rate-limit pin for `warn_checkpoint_failure` itself (observability
/// garnish). Does **not** couple to `checkpoint_loop`'s rescue wire —
/// deleting the rescue call would keep this green; takeover pins flock.
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
  Db.send(:warn_checkpoint_failure, RuntimeError.new("first"))
  Db.send(:warn_checkpoint_failure, RuntimeError.new("second"))
  Db.instance_variable_set(
    :@checkpoint_warn_at,
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - Db::CHECKPOINT_WARN_INTERVAL - 1
  )
  Db.send(:warn_checkpoint_failure, RuntimeError.new("third"))
ensure
  $stderr = real
end
out = buf.string
check("first failure warned", out.include?("first"))
check("second failure suppressed inside the interval", !out.include?("second"))
check("third failure warned after the interval", out.include?("third"))
check("warn names the checkpoint", out.include?("WAL checkpoint failed"))
# A raising warning sink must not escape — otherwise checkpoint_loop dies
# after wal_autocheckpoint=0 with no restart.
Db.instance_variable_set(:@checkpoint_warn_at, nil)
def Kernel.warn(*)
  raise IOError, "broken pipe"
end
begin
  Db.send(:warn_checkpoint_failure, RuntimeError.new("sink"))
rescue StandardError => e
  raise "warn sink escaped: #{e.class}: #{e.message}"
end
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
begin
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
  leader = nil
  check("leader exited", status.signaled?)

  deadline = Time.now + 5
  sleep 0.05 while File.size(path) <= baseline + 400_000 && Time.now < deadline
  check("survivor checkpointer took over and copied (#{baseline} -> #{File.size(path)})",
        File.size(path) > baseline + 400_000)
ensure
  if leader
    begin
      Process.kill("KILL", leader)
    rescue Errno::ESRCH
    end
    begin
      Process.wait2(leader)
    rescue Errno::ECHILD
    end
  end
end
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
      rescue ActiveRecord::StatementTimeout
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
    for name in ["db.rb", "active_support_time_parsing.rb", "active_support_interval.rb", "active_support_duration.rb"] {
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
