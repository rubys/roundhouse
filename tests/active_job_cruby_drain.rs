//! The CRuby overlay's job drain (`active_job_cruby.rb`): once
//! `config.ru` calls `ActiveJob.drain_in_background!`, a lowered
//! `perform_later` queues its work and returns, and a drain thread in
//! the same process runs it under its own Db lease. Checked: the caller
//! returns before a slow job finishes; every job enqueued from many
//! threads runs exactly once, off the callers' threads; a forked child
//! starts its own drain (threads do not survive fork); and the test
//! log stops growing while serving. Before any registration, jobs run
//! inline as the emitted test harness expects.

use std::path::Path;
use std::process::Command;

#[test]
fn queued_jobs_run_on_a_drain_thread_per_process() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
LEASES = []
module Db
  def self.with_connection
    LEASES << Thread.current
    yield
  end
end
require_relative "runtime/ruby/active_job"
require_relative "runtime/spinel/thread_state"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/active_job_cruby"

def wait_for(secs = 5)
  t0 = Time.now
  until yield
    raise "timed out" if Time.now - t0 > secs
    sleep 0.005
  end
end

raise "drain registered before serving" if ActiveJob.drain_registered
ActiveJob.record_performed("Before")
raise "test log not recorded" unless ActiveJob.performed == ["Before"]

ActiveJob.drain_in_background!
raise "drain not registered" unless ActiveJob.drain_registered

# The caller returns before a slow job finishes.
done = Queue.new
t0 = Time.now
ActiveJob.enqueue(-> { sleep 0.3; done << Thread.current; nil })
raise "enqueue blocked #{Time.now - t0}s" if Time.now - t0 > 0.1
ran_on = done.pop
raise "job ran on the caller's thread" if ran_on == Thread.current
raise "job ran outside a Db lease" unless LEASES.include?(ran_on)

# Many enqueuing threads: each job runs once, all on the drain thread.
seen = Queue.new
callers = 8.times.map do |t|
  Thread.new { 50.times { |i| ActiveJob.enqueue(-> { seen << [t, i, Thread.current]; nil }) } }
end
callers.each(&:join)
wait_for { seen.size == 400 }
sleep 0.05
raise "extra runs: #{seen.size}" unless seen.size == 400
got = []
got << seen.pop until seen.empty?
raise "duplicate or missing" unless got.map { |t, i, _| [t, i] }.uniq.size == 400
raise "ran on more than one thread" unless got.map(&:last).uniq == [ran_on]

# A raising job is one lost job, not a lost drain.
ActiveJob.enqueue(-> { raise "boom" })
after = Queue.new
ActiveJob.enqueue(-> { after << :ok; nil })
raise "drain died after a raise" unless after.pop == :ok

# Serving: the test log no longer grows.
ActiveJob.record_performed("While serving")
raise "log grew while serving" unless ActiveJob.performed == ["Before"]

# A forked child (a Puma worker) drains in its own thread.
r, w = IO.pipe
pid = fork do
  r.close
  q = Queue.new
  ActiveJob.enqueue(-> { q << Process.pid; nil })
  w.puts(q.pop == Process.pid ? "child ok" : "child wrong")
  w.close
  exit!(0)
end
w.close
line = nil
th = Thread.new { line = r.gets }
th.join(5) or raise "child drain never ran"
Process.wait(pid)
raise "child: #{line.inspect}" unless line&.strip == "child ok"
puts "ALL OK"
"##;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "job drain failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
