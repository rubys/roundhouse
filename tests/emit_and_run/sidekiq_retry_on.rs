//! `retry_on`, `discard_on` and `sidekiq_options` on ActiveJob classes
//! under `queue_adapter :sidekiq`: an emitted tree runs each popped job
//! as Sidekiq's ActiveJob wrapper does, and pushes what ActiveJob 8.1
//! and Sidekiq 8.1 write. The expected writes are the oracle's
//! (`tests/sidekiq_worker/activejob.json`, from `aj_oracle.rb`), with
//! the same jobs.

use roundhouse::project::BuildTarget;

use super::emit_and_run;

const ERRORS: &str = "class Sticky < StandardError\nend\n\nclass Slippery < StandardError\nend\n\nclass Gone < StandardError\nend\n";

const APPLICATION_JOB: &str = r##"class ApplicationJob < ActiveJob::Base
  discard_on ActiveJob::DeserializationError
end
"##;

const JOBS: &[(&str, &str)] = &[
    (
        "fixed_wait_job",
        "class FixedWaitJob < ApplicationJob\n  retry_on Sticky, wait: 1, attempts: 3\n\n  def perform(name)\n    raise Sticky, name + \" sticks\"\n  end\nend\n",
    ),
    (
        "poly_job",
        "class PolyJob < ApplicationJob\n  retry_on Sticky, Slippery, wait: :polynomially_longer, attempts: 4\n\n  def perform(name)\n    raise Slippery, name + \" slips\"\n  end\nend\n",
    ),
    (
        "duration_job",
        "class DurationJob < ApplicationJob\n  retry_on Sticky, wait: 5.minutes, attempts: :unlimited, queue: :low, priority: 5\n\n  def perform(name)\n    raise Sticky, name + \" sticks\"\n  end\nend\n",
    ),
    (
        "class_jitter_job",
        "class ClassJitterJob < ApplicationJob\n  self.retry_jitter = 0.15\n  retry_on Sticky, wait: 10, attempts: 2\n  retry_on Slippery, wait: :polynomially_longer, attempts: 2\n\n  def perform(name)\n    raise Slippery, name if name == \"slip\"\n    raise Sticky, name\n  end\nend\n",
    ),
    (
        "handler_jitter_job",
        "class HandlerJitterJob < ApplicationJob\n  self.retry_jitter = 0.15\n  retry_on Sticky, wait: 10, jitter: 0.5, attempts: 2\n\n  def perform(name)\n    raise Sticky, name\n  end\nend\n",
    ),
    (
        "discard_job",
        "class DiscardJob < ApplicationJob\n  discard_on Gone\n\n  def perform(name)\n    raise Gone, name + \" is gone\"\n  end\nend\n",
    ),
    (
        "order_job",
        "class OrderJob < ApplicationJob\n  retry_on Sticky, wait: 1\n  discard_on Sticky\n\n  def perform(name)\n    raise Sticky, name\n  end\nend\n",
    ),
    (
        "options_job",
        "class OptionsJob < ApplicationJob\n  sidekiq_options retry: 3, retry_queue: \"low\"\n  retry_on Sticky, wait: 1, attempts: 2\n\n  def perform(name)\n    raise Sticky, name\n  end\nend\n",
    ),
    (
        "no_handler_job",
        "class NoHandlerJob < ActiveJob::Base\n  def perform(name)\n    raise Sticky, name\n  end\nend\n",
    ),
    ("arg_job", "class ArgJob < ApplicationJob\n  def perform(article)\n    article.title\n  end\nend\n"),
    (
        "arg_no_handler_job",
        "class ArgNoHandlerJob < ActiveJob::Base\n  def perform(article)\n    article.title\n  end\nend\n",
    ),
];

const ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_retry_jobs
    FixedWaitJob.perform_later(title.to_s)
    PolyJob.perform_later(title.to_s)
    DurationJob.perform_later(title.to_s)
    ClassJitterJob.perform_later(title.to_s)
    HandlerJitterJob.perform_later(title.to_s)
    DiscardJob.perform_later(title.to_s)
    OrderJob.perform_later(title.to_s)
    OptionsJob.perform_later(title.to_s)
    NoHandlerJob.perform_later(title.to_s)
    ArgJob.perform_later(self)
    ArgNoHandlerJob.perform_later(self)
  end
"##;

const SIDEKIQ: &str = "  config.active_job.queue_adapter = :sidekiq\n";

/// The oracle loads no Rails app, so its `retry_jitter` is ActiveJob's
/// own 0.0 rather than the 0.15 `load_defaults` 6.1+ sets.
const NO_JITTER: &str = "  config.active_job.queue_adapter = :sidekiq\n  config.active_job.retry_jitter = 0.0\n";

fn on_sidekiq() -> emit_and_run::Overlay {
    jobs().edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", NO_JITTER)
}

fn jobs() -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::real_blog()
        .write("app/models/sticky.rb", ERRORS)
        .write("app/jobs/application_job.rb", APPLICATION_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE);
    for (file, source) in JOBS {
        overlay = overlay.write(&format!("app/jobs/{file}.rb"), source);
    }
    overlay
}

/// Each scenario of the oracle: the job's writer suffix, its argument
/// (Ruby), and the oracle's steps.
const SCENARIOS: &[(&str, &str, &str)] = &[
    ("fixed_wait", "fixed_wait_job", "\"fixed\""),
    ("polynomially_longer_two_exceptions", "poly_job", "\"poly\""),
    ("duration_unlimited_queue_priority", "duration_job", "\"dur\""),
    ("class_jitter_fixed", "class_jitter_job", "\"stick\""),
    ("class_jitter_polynomially_longer", "class_jitter_job", "\"slip\""),
    ("handler_jitter", "handler_jitter_job", "\"hj\""),
    ("discard_on", "discard_job", "\"gone\""),
    ("search_order", "order_job", "\"order\""),
    ("sidekiq_options_then_exhausted", "options_job", "\"opts\""),
    ("no_handler", "no_handler_job", "\"none\""),
];

fn ruby(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Ruby's `Float#to_s` for the delays the oracle has.
fn ruby_float(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") }
}

/// One write, as the script checks it: where, the delay, the envelope's
/// keys, `queue`, `retry`, `retry_queue`, and the ActiveJob payload's
/// `executions`, `exception_executions`, `queue_name` and `priority`.
fn write_expectation(wrote: &serde_json::Value) -> String {
    let env = &wrote["envelope"];
    let aj = &env["args"][0];
    let text = |v: &serde_json::Value| match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let to = wrote["to"].as_str().unwrap();
    let to = if to == "schedule" { "schedule" } else { "queue" };
    let keys: Vec<&str> = wrote["envelope_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
    let fields = [
        to.to_string(),
        wrote.get("score_minus_now").and_then(|v| v.as_f64()).map(ruby_float).unwrap_or_default(),
        keys.join(","),
        text(&env["queue"]),
        text(&env["retry"]),
        text(&env["retry_queue"]),
        text(&aj["executions"]),
        aj["exception_executions"].to_string(),
        text(&aj["queue_name"]),
        text(&aj["priority"]),
    ];
    format!("[{}]", fields.iter().map(|f| ruby(f)).collect::<Vec<_>>().join(", "))
}

fn expectations() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sidekiq_worker/activejob.json");
    let oracle: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(oracle["default_retry_jitter"], 0.0);
    assert_eq!(oracle["kernel_rand"], 0.5);
    let mut out = String::from("SCENARIOS = [\n");
    for (name, job, arg) in SCENARIOS {
        let steps = oracle["scenarios"][*name].as_array().unwrap_or_else(|| panic!("{name}"));
        let mut lines = Vec::new();
        for step in steps {
            if let Some(pushed) = step.get("pushed") {
                lines.push(write_expectation(pushed));
            } else if let Some(wrote) = step.get("wrote") {
                lines.push(write_expectation(wrote));
            } else if let Some(raised) = step["raised"].as_str() {
                lines.push(format!("[\"raised\", {}]", ruby(raised)));
            } else {
                lines.push("[\"nothing\"]".to_string());
            }
        }
        out.push_str(&format!("  [{}, {}, {arg}, [\n    {}\n  ]],\n", ruby(name), ruby(job), lines.join(",\n    ")));
    }
    out.push_str("]\n");
    let reruns: Vec<String> = oracle["exhausted_payload_reruns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| match r["raised"].as_str() {
            Some(m) => format!("[\"raised\", {}]", ruby(m)),
            None => "[\"wrote\"]".to_string(),
        })
        .collect();
    out.push_str(&format!("RERUNS = [{}]\n", reruns.join(", ")));
    out
}

const SCRIPT: &str = r##"module ActiveJob
  def self.retry_rand
    0.5
  end
end

FAILS = [0]
CHECKS = [0]

def check(name, ok, detail)
  CHECKS[0] += 1
  return if ok
  FAILS[0] += 1
  puts "FAIL " + name + " (" + detail + ")"
end

# The one push a run made, as [where, at, envelope text]; at is 0.0 for
# a queue.
def take_write(name)
  rec = SidekiqPush.recorded
  SidekiqPush.stop_recording
  write = ["", "0.0", ""]
  rec.each do |line|
    parts = line.split(" ", 4)
    if parts[0] == "zadd"
      write = ["schedule", parts[2], parts[3]]
    elsif parts[0] == "lpush"
      write = ["queue", "0.0", line.split(" ", 3)[2]]
    end
  end
  write
end

def describe(name, write, now, expect)
  env = JSON.parse(write[2])
  aj = env["args"][0]
  check(name + " to", write[0] == expect[0], write[0])
  if write[0] == "schedule"
    delay = write[1].to_f - now
    check(name + " delay", (delay - expect[1].to_f).abs < 0.05, delay.to_s)
  end
  check(name + " keys", env.keys.join(",") == expect[2], env.keys.join(","))
  check(name + " queue", env["queue"].to_s == expect[3], env["queue"].to_s)
  check(name + " retry", env["retry"].to_s == expect[4], env["retry"].to_s)
  check(name + " retry_queue", env["retry_queue"].to_s == expect[5], env["retry_queue"].to_s)
  check(name + " executions", aj["executions"].to_s == expect[6], aj["executions"].to_s)
  check(name + " exception_executions", JSON.generate(aj["exception_executions"]) == expect[7],
    JSON.generate(aj["exception_executions"]))
  check(name + " queue_name", aj["queue_name"].to_s == expect[8], aj["queue_name"].to_s)
  check(name + " priority", aj["priority"].to_s == expect[9], aj["priority"].to_s)
end

# Run the envelope's job as the worker does; answer the next write's
# text, or "" when it made none.
def run_once(name, raw, expect)
  env = JSON.parse(raw)
  jid = env["jid"].to_s
  SidekiqPush.start_recording
  now = Time.now.to_f
  outcome = ActiveJob.perform_parsed_from(env["args"][0], jid)
  write = take_write(name)
  if expect[0] == "raised"
    check(name + " raised", outcome == ActiveJob::PERFORM_FAILED, outcome.to_s)
    msg = ActiveJob.failure_class + ": " + ActiveJob.failure_message
    want = expect[1]
    ok = want.end_with?("*") ? msg.start_with?(want[0, want.length - 1]) : msg == want
    check(name + " error", ok, msg)
    check(name + " no write", write[0] == "", write[0])
    return ""
  end
  if expect[0] == "nothing"
    check(name + " discarded", outcome == ActiveJob::PERFORM_DISCARDED, outcome.to_s)
    check(name + " no write", write[0] == "", write[0])
    return ""
  end
  check(name + " retried", outcome == ActiveJob::PERFORM_RETRIED, outcome.to_s)
  describe(name, write, now, expect)
  check(name + " provider_job_id", JSON.parse(write[2])["args"][0]["provider_job_id"].to_s == jid, "")
  write[2]
end

article = Article.create!(title: "x", body: "A long enough article body.")
SCENARIOS.each do |scenario|
  name = scenario[0]
  steps = scenario[3]
  SidekiqPush.start_recording
  if scenario[1] == "fixed_wait_job"
    JobRegistry.push_fixed_wait_job("", 0.0, scenario[2])
  elsif scenario[1] == "poly_job"
    JobRegistry.push_poly_job("", 0.0, scenario[2])
  elsif scenario[1] == "duration_job"
    JobRegistry.push_duration_job("", 0.0, scenario[2])
  elsif scenario[1] == "class_jitter_job"
    JobRegistry.push_class_jitter_job("", 0.0, scenario[2])
  elsif scenario[1] == "handler_jitter_job"
    JobRegistry.push_handler_jitter_job("", 0.0, scenario[2])
  elsif scenario[1] == "discard_job"
    JobRegistry.push_discard_job("", 0.0, scenario[2])
  elsif scenario[1] == "order_job"
    JobRegistry.push_order_job("", 0.0, scenario[2])
  elsif scenario[1] == "options_job"
    JobRegistry.push_options_job("", 0.0, scenario[2])
  else
    JobRegistry.push_no_handler_job("", 0.0, scenario[2])
  end
  pushed = take_write(name)
  describe(name + " pushed", pushed, 0.0, steps[0])
  raw = pushed[2]
  i = 1
  while i < steps.length && raw != ""
    raw = run_once(name + " run " + i.to_s, raw, steps[i])
    i += 1
  end
  check(name + " ran every step", i == steps.length, i.to_s)
end

# Sidekiq retrying an exhausted OptionsJob runs the last payload
# ActiveJob scheduled: it counts to the limit again and raises at once.
SidekiqPush.start_recording
JobRegistry.push_options_job("", 0.0, "again")
raw = take_write("again")[2]
RERUNS.each_with_index do |expect, i|
  if expect[0] == "wrote"
    SidekiqPush.start_recording
    outcome = ActiveJob.perform_parsed_from(JSON.parse(raw)["args"][0], JSON.parse(raw)["jid"].to_s)
    write = take_write("rerun")
    check("rerun " + i.to_s + " retried", outcome == ActiveJob::PERFORM_RETRIED && write[0] == "schedule", outcome.to_s)
    raw = write[2]
  else
    run_once("rerun " + i.to_s, raw, expect)
  end
end

# A record that is gone: ApplicationJob's `discard_on` discards it, a
# job without one fails with DeserializationError.
gone = Article.create!(title: "gone", body: "A long enough article body.")
SidekiqPush.start_recording
JobRegistry.push_arg_job("", 0.0, gone)
JobRegistry.push_arg_no_handler_job("", 0.0, gone)
rec = SidekiqPush.recorded
SidekiqPush.stop_recording
gone.destroy
envs = []
rec.each { |line| envs.push(line.split(" ", 3)[2]) if line.start_with?("lpush") }
check("two record pushes", envs.length == 2, envs.length.to_s)
run_once("deserialization discarded", envs[0], ["nothing"])
run_once("deserialization no handler", envs[1],
  ["raised", "ActiveJob::DeserializationError: *"])
article.destroy
puts "retry_on passed " + CHECKS[0].to_s + " checks" if FAILS[0] == 0
"##;

/// What the emitted tree's `retry_on`, `discard_on` and
/// `sidekiq_options` write, run by CRuby, against the oracle.
#[test]
fn retry_on_writes_what_active_job_does() {
    let script = format!("{}\n{SCRIPT}", expectations());
    let run = on_sidekiq().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("retry_on passed"), "{}", run.stdout);
    let registry = std::fs::read_to_string(run.emitted.join("runtime/job_registry.rb")).unwrap();
    assert!(registry.contains("return 8 if e.is_a?(Sticky) || e.is_a?(Slippery)"), "{registry}");
    let job = std::fs::read_to_string(run.emitted.join("app/models/options_job.rb"))
        .or_else(|_| std::fs::read_to_string(run.emitted.join("app/jobs/options_job.rb")))
        .unwrap();
    assert!(!job.contains("sidekiq_options") && !job.contains("retry_on"), "{job}");
}

/// Every handler the oracle's jobs declare is carried, so none is
/// reported as a dropped class-body call.
#[test]
fn carried_handlers_report_nothing() {
    let (_, errors) = on_sidekiq().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The fixture's `load_defaults` is 6.1 or later, so a handler without
/// `jitter:` on a job without `retry_jitter` gets Rails' 0.15; the job's
/// own and the handler's win over it.
#[test]
fn load_defaults_sets_the_retry_jitter() {
    let (emitted, errors) = jobs()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let registry = std::fs::read_to_string(emitted.join("runtime/job_registry.rb")).unwrap();
    assert!(
        registry.contains("ActiveJob::RetryRule.new(\"[Sticky]\", 1.0, false, 3, \"\", false, 0, 0.15)"),
        "{registry}"
    );
    assert!(registry.contains("ActiveJob::RetryRule.new(\"[Sticky]\", 10.0, false, 2, \"\", false, 0, 0.5)"));
}

const UNCARRIED: &str = r##"class UncarriedJob < ApplicationJob
  retry_on Sticky, wait: ->(n) { n * 2 }
  retry_on Net::ReadTimeout
  discard_on(Gone) { |job, error| job.title }
  discard_on Sticky, report: true

  def perform(name)
    name
  end
end
"##;

/// Under `:sidekiq` a handler the runtime can't carry is an error at
/// its call, naming why; on the in-process queue it stays a dropped
/// call, as before.
#[test]
fn an_uncarried_handler_is_an_error_only_under_sidekiq() {
    let overlay = || {
        jobs()
            .write("app/jobs/uncarried_job.rb", UNCARRIED)
            .edit("app/models/article.rb", "  def enqueue_retry_jobs\n", "  def enqueue_retry_jobs\n    UncarriedJob.perform_later(title.to_s)\n")
    };
    let (_, errors) = overlay()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert_eq!(errors.len(), 4, "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("`discard_on`'s `report: true` isn't supported")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("`retry_on`'s `wait:` isn't a literal")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("`Net::ReadTimeout` isn't an exception class")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("a block on `discard_on` isn't supported")), "{errors:#?}");

    let (_, errors) = overlay().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}
