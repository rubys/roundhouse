//! `ActiveJob.perform_held` (the blockless `perform_enqueued_jobs`) walks
//! a snapshot of what is held, as Rails' `flush_enqueued_jobs` does
//! (`jobs.dup`).
//!
//! A job-iteration job interrupted by `mark_job_worker_as_interrupted`
//! re-enqueues itself, and a pass that ran what its jobs enqueue never
//! finished: a large app's an import job's test "translation counter is
//! incremented as expected even when the job is interrupted" ran for more
//! than 40 minutes. The runtime file is loaded by itself, as the CRuby
//! emit carries it.

use std::path::Path;
use std::process::Command;

const DRIVER: &str = r#"
require "./active_job"

ActiveJob.enqueue_without_running

# A job that holds a copy of itself, as an interrupted iteration job does.
runs = 0
reenqueue = nil
reenqueue = lambda do
  ActiveJob.hold("InterruptedJob", lambda do
    runs += 1
    reenqueue.call
    nil
  end)
end
reenqueue.call

ran = ActiveJob.perform_held(["InterruptedJob"])
puts "pass1 ran=#{ran} runs=#{runs} held=#{ActiveJob::HELD.length}"
ran = ActiveJob.perform_held([])
puts "pass2 ran=#{ran} runs=#{runs} held=#{ActiveJob::HELD.length}"

# `only` leaves other jobs held, and the snapshot is of those that were
# held at the call: a job held ahead of one that runs keeps its place.
ActiveJob.clear_held
order = []
ActiveJob.hold("A", lambda { order << "A"; nil })
ActiveJob.hold("B", lambda { order << "B"; ActiveJob.hold("B", lambda { order << "B2"; nil }); nil })
ActiveJob.hold("C", lambda { order << "C"; nil })
ran = ActiveJob.perform_held(["B", "C"])
puts "only ran=#{ran} order=#{order.join(",")} held=#{ActiveJob::HELD_NAMES.join(",")}"
ran = ActiveJob.perform_held([])
puts "rest ran=#{ran} order=#{order.join(",")} held=#{ActiveJob::HELD.length}"
"#;

fn run() -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("active-job-perform-held-snapshot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/active_job.rb"),
        dir.join("active_job.rb"),
    )
    .expect("copy runtime source");
    let mut child = Command::new("ruby")
        .args(["-W0", "-e", DRIVER])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("ruby is on PATH");
    // A regression loops forever, so the driver gets a bound.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if child.try_wait().expect("wait").is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("perform_held did not return: a job it enqueued ran in the same pass");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().expect("output");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_job_that_reenqueues_itself_runs_once_per_pass() {
    assert_eq!(
        run(),
        "pass1 ran=1 runs=1 held=1\n\
         pass2 ran=1 runs=2 held=1\n\
         only ran=2 order=B,C held=A,B\n\
         rest ran=2 order=B,C,A,B2 held=0\n"
    );
}
