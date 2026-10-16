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

# Each job starts with an empty broadcast log: the drain thread is never
# a request, so nothing else would clear what its jobs broadcast.
sizes = Queue.new
3.times { ActiveJob.enqueue(-> { Broadcasts.log_append({ html: "x" }); sizes << Broadcasts.log.length; nil }) }
got_sizes = 3.times.map { sizes.pop }
raise "broadcast log grew across jobs: #{got_sizes.inspect}" unless got_sizes == [1, 1, 1]

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

/// Payload jobs (`lower::job_payload`) take the same drain: enqueued
/// from many threads, each runs exactly once, in FIFO order with the
/// Proc jobs around it, and a forked child drains its own. The job
/// registry here is a hand-written stand-in for the generated one.
#[test]
fn payload_jobs_run_on_the_same_drain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "json"
require "securerandom"
require "time"
module Db
  def self.with_connection
    yield
  end
end
require_relative "runtime/ruby/active_job"
require_relative "runtime/spinel/active_job_serialization"
require_relative "runtime/spinel/job_registry"
require_relative "runtime/spinel/thread_state"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/active_job_cruby"

RAN = Queue.new
module JobRegistry
  def self.perform(job_class, args)
    return false unless job_class == "CountJob"
    RAN << [ActiveJob::Arguments.int_at(args, 0), ActiveJob::Arguments.str_at(args, 1), Thread.current]
    true
  end
end

def payload(n, tag)
  ActiveJob::Payload.build("CountJob", "default",
    ActiveJob::Arguments.list([ActiveJob::Arguments.int(n), ActiveJob::Arguments.str(tag)]))
end

def wait_for(secs = 5)
  t0 = Time.now
  until yield
    raise "timed out" if Time.now - t0 > secs
    sleep 0.005
  end
end

ActiveJob.drain_in_background!

callers = 8.times.map do |t|
  Thread.new { 50.times { |i| ActiveJob.enqueue_payload(payload(t * 100 + i, "t#{t}")) } }
end
callers.each(&:join)
wait_for { RAN.size == 400 }
sleep 0.05
raise "extra runs: #{RAN.size}" unless RAN.size == 400
got = []
got << RAN.pop until RAN.empty?
raise "duplicate or missing" unless got.map(&:first).uniq.size == 400
raise "ran on the caller" if got.map(&:last).include?(Thread.current)
raise "argument lost" unless got.all? { |n, tag, _| tag == "t#{n / 100}" }

# FIFO across the two kinds.
ORDER_SEEN = Queue.new
module JobRegistry
  def self.perform(job_class, args)
    return false unless job_class == "CountJob"
    ORDER_SEEN << "j#{ActiveJob::Arguments.int_at(args, 0)}"
    true
  end
end
ActiveJob.enqueue(-> { sleep 0.05; ORDER_SEEN << "p1"; nil })
ActiveJob.enqueue_payload(payload(2, "x"))
ActiveJob.enqueue(-> { ORDER_SEEN << "p3"; nil })
ActiveJob.enqueue_payload(payload(4, "x"))
seen = 4.times.map { ORDER_SEEN.pop }
raise "order #{seen.inspect}" unless seen == ["p1", "j2", "p3", "j4"]

# An unknown class and a raising payload are lost jobs, not a lost drain.
ActiveJob.enqueue_payload(ActiveJob::Payload.build("NopeJob", "default", "[]"))
ActiveJob.enqueue_payload(ActiveJob::Payload.build("CountJob", "default", "[{}]"))
after = Queue.new
ActiveJob.enqueue(-> { after << :ok; nil })
raise "drain died" unless after.pop == :ok

# A forked child drains payloads in its own thread.
r, w = IO.pipe
pid = fork do
  r.close
  q = Queue.new
  module JobRegistry
    def self.perform(job_class, args)
      Q_CHILD << Process.pid
      true
    end
  end
  Q_CHILD = q
  ActiveJob.enqueue_payload(payload(1, "child"))
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
        "payload drain failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

/// A server can run the drain itself (falcon.rb makes it a task on the
/// worker's event loop): `start_drainer` hands the runner the drain loop
/// instead of starting a thread, later enqueues use that drain, and a
/// forked child, where no runner was given, falls back to a thread.
#[test]
fn a_server_supplied_runner_hosts_the_drain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
module Db
  def self.with_connection
    yield
  end
end
require_relative "runtime/ruby/active_job"
require_relative "runtime/spinel/thread_state"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/active_job_cruby"

ActiveJob.drain_in_background!
host = nil
ActiveJob.start_drainer { |work| host = Thread.new { Thread.current.name = "runner"; work.call } }
raise "runner was not given the drain" unless host
threads_before = Thread.list.size
ran = Queue.new
5.times { |i| ActiveJob.enqueue(-> { ran << [i, Thread.current.name]; nil }) }
got = 5.times.map { ran.pop }
raise "jobs did not run on the runner: #{got.inspect}" unless got.map(&:last).uniq == ["runner"]
raise "jobs out of order: #{got.inspect}" unless got.map(&:first) == [0, 1, 2, 3, 4]
raise "enqueue started another drain" unless Thread.list.size == threads_before
second = false
ActiveJob.start_drainer { |_| second = true }
raise "a second start_drainer replaced the drain" if second

r, w = IO.pipe
pid = fork do
  r.close
  q = Queue.new
  ActiveJob.enqueue(-> { q << Thread.current.name; nil })
  w.puts(q.pop == "runner" ? "child used the parent's runner" : "child ok")
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
        "runner-hosted drain failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
