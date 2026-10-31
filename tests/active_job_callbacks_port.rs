//! PostgreSQL: active job callbacks port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;
use std::path::PathBuf;
use std::process::Command;
const APPLICATION_JOB: &str = r#"class ApplicationJob < ActiveJob::Base
  TRAIL = []

  def trail
    TRAIL
  end
end
"#;
const TRACED: &str = r#"module Traced
  extend ActiveSupport::Concern

  included do
    before_perform { |job| job.trail << "traced before #{job.arguments.first}" }
    around_perform do |job, block|
      trail << "traced around in"
      block.call
      trail << "traced around out"
    end
    before_enqueue :note_enqueue
    rescue_from(KeyError) { |e| trail << "traced rescued #{e.message}" }
  end

  def note_enqueue
    trail << "traced before_enqueue"
  end
end
"#;
/// A Concern that carries a class attribute and an accessor onto the job.
const STAMPED: &str = r#"module Stamped
  extend ActiveSupport::Concern

  included do
    class_attribute :stamp, default: "plain"
    attr_accessor :note
  end
end
"#;
const FALLBACK: &str = r#"module Fallback
  class << self
    def included(base)
      base.around_enqueue do |job, block|
        job.trail << "fallback in"
        block.call
        job.trail << "fallback out"
      end
    end
  end
end
"#;
/// A job that includes all three and registers the rest of the surface
/// in its own body: `prepend:`, a symbol around, a halting
/// `before_enqueue`, `after_enqueue`, `rescue_from ..., with:` and by
/// class name.
const REPORT_JOB: &str = r#"class ReportJob < ApplicationJob
  include Traced
  include Stamped
  include Fallback

  self.stamp = "report"

  before_perform :own_before
  before_perform(prepend: true) { |job| job.trail << "prepended" }
  after_perform { |job| job.trail << "after #{job.arguments.inspect}" }
  around_perform :own_around
  before_enqueue do |job|
    trail << "before_enqueue #{job.arguments.first}"
    throw :abort if job.arguments.first == :refuse
  end
  after_enqueue { trail << "after_enqueue" }
  rescue_from ArgumentError, with: :handle_argument
  rescue_from "RangeError" do |e|
    trail << "range #{e.message}"
  end

  def perform(kind, count = 1)
    trail << "perform #{kind} #{count}"
    case kind
    when :argument then raise ArgumentError, "bad"
    when :range then raise RangeError, "far"
    when :key then raise KeyError, "key"
    when :wrapped
      begin
        raise KeyError, "inner"
      rescue KeyError
        raise IOError, "outer"
      end
    when :io then raise IOError, "io"
    end
    "done #{kind}"
  end

  private

  def own_before
    trail << "own before"
  end

  def own_around
    trail << "own around in"
    yield
    trail << "own around out"
  end

  def handle_argument(error)
    trail << "handled #{error.message}"
  end
end
"#;
/// A subclass that skips a callback and halts in a `before_perform`.
const HALTED_JOB: &str = r#"class HaltedJob < ReportJob
  skip_callback :perform, :before, :own_before
  before_perform { |job| throw :abort if job.arguments.first == :halt }
end
"#;
const TRACK_JOB: &str = "module ShopEvent\n  class TrackJob < ApplicationJob\n    def perform(kind)\n      trail << \"track #{kind}\"\n    end\n  end\nend\n";

const TRACK_CREATE_JOB: &str =
    "module ShopEvent\n  class TrackCreateJob < TrackJob\n    include Traced\n  end\nend\n";

/// A job outside the hierarchy, with no callbacks: it keeps the
/// generated wrappers.
const PLAIN_JOB: &str = r#"class PlainJob < ActiveJob::Base
  def perform(value)
    value
  end
end
"#;
/// The observations, run after the files load; `hold_jobs`, `held_count`
/// and `run_held` are each side's test adapter.
const OBSERVE: &str = r#"def run(label)
  ApplicationJob::TRAIL.clear
  result = begin
    yield
  rescue Exception => e
    [:raised, e.class.name, e.message]
  end
  result = result.class.name if result.is_a?(ActiveJob::Base)
  result = [result.class.name, result.message] if result.is_a?(Exception)
  p [label, result, ApplicationJob::TRAIL.dup]
end
run(:now) { ReportJob.perform_now(:ok, 2) }
run(:argument) { ReportJob.perform_now(:argument) }
run(:range) { ReportJob.perform_now(:range) }
run(:key) { ReportJob.perform_now(:key) }
run(:wrapped) { ReportJob.perform_now(:wrapped) }
run(:io) { ReportJob.perform_now(:io) }
run(:later) { ReportJob.perform_later(:ok) }
run(:refused) { ReportJob.perform_later(:refuse) }
run(:halted) { HaltedJob.perform_now(:halt) }
run(:halted_ok) { HaltedJob.perform_now(:ok) }
run(:halted_later) { HaltedJob.perform_later(:halt) }
run(:nested) { ShopEvent::TrackCreateJob.perform_now(:create) }
run(:nested_parent) { ShopEvent::TrackJob.perform_later(:plain) }
run(:instance) { job = ReportJob.new(:ok); [job.perform_now, job.executions, job.arguments] }
p [ReportJob.stamp, HaltedJob.stamp, ReportJob.new.stamp, ApplicationJob.respond_to?(:stamp)]
p [ReportJob._perform_callbacks.map(&:kind), HaltedJob._perform_callbacks.count, ReportJob._enqueue_callbacks.map(&:kind).last(4)]
p ReportJob.rescue_handlers.map(&:first)
p PlainJob.perform_now(5)
# `ActiveJob.perform_all_later`: no enqueue callbacks (the halting one is skipped), jobs flattened.
run(:all_later) { jobs = [ReportJob.new(:ok, 1), ReportJob.new(:refuse)]; [ActiveJob.perform_all_later(jobs), jobs.map(&:successfully_enqueued?)] }
run(:all_later_splat) { job = ReportJob.new(:ok, 2); [ActiveJob.perform_all_later(job, [HaltedJob.new(:ok)]), job.successfully_enqueued?] }
hold_jobs
run(:held) { ReportJob.perform_later(:ok, 3) }
run(:held_refused) { ReportJob.perform_later(:refuse) }
p held_count
run(:drained) { run_held }
p held_count
"#;
/// The native `:test` adapter: what `queue_adapter.enqueued_jobs` holds,
/// run as the adapter's `perform_enqueued_jobs` runs them.
const NATIVE_ADAPTER: &str = r#"def hold_jobs
  ActiveJob::Base.queue_adapter = :test
end

def held_count
  ActiveJob::Base.queue_adapter.enqueued_jobs.size
end

def run_held
  jobs = ActiveJob::Base.queue_adapter.enqueued_jobs.dup
  ActiveJob::Base.queue_adapter.enqueued_jobs.clear
  jobs.each { |job| ActiveJob::Base.execute(job) }
  jobs.size
end
"#;

const EMITTED_ADAPTER: &str = r#"def hold_jobs
  ActiveJob.enqueue_without_running
end

def held_count
  ActiveJob::HELD.length
end

def run_held
  ActiveJob.perform_held([])
end
"#;

const FILES: &[(&str, &str, &str)] = &[
    (
        "app/jobs/application_job.rb",
        "app/models/application_job.rb",
        APPLICATION_JOB,
    ),
    (
        "app/jobs/concerns/traced.rb",
        "app/models/traced.rb",
        TRACED,
    ),
    (
        "app/jobs/concerns/stamped.rb",
        "app/models/stamped.rb",
        STAMPED,
    ),
    ("app/jobs/fallback.rb", "app/models/fallback.rb", FALLBACK),
    (
        "app/jobs/report_job.rb",
        "app/models/report_job.rb",
        REPORT_JOB,
    ),
    (
        "app/jobs/halted_job.rb",
        "app/models/halted_job.rb",
        HALTED_JOB,
    ),
    (
        "app/jobs/shop_event/track_job.rb",
        "app/models/shop_event/track_job.rb",
        TRACK_JOB,
    ),
    (
        "app/jobs/shop_event/track_create_job.rb",
        "app/models/shop_event/track_create_job.rb",
        TRACK_CREATE_JOB,
    ),
    (
        "app/jobs/plain_job.rb",
        "app/models/plain_job.rb",
        PLAIN_JOB,
    ),
];

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app().write("roundhouse.yml", "test_paths:\n  - test\n");
    for (path, _, text) in FILES {
        app = app.write(path, text);
    }
    app
}

fn requires() -> String {
    FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect()
}

fn native() -> String {
    let files: Vec<(&str, &str)> = FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    native_of(&files, &format!("{}{NATIVE_ADAPTER}{OBSERVE}", requires()))
}

fn native_of(files: &[(&str, &str)], program: &str) -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rh_active_job_callbacks_native_{}_{}",
        std::process::id(),
        RUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    for (emitted, text) in files {
        let file = dir.join(emitted);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activesupport\", \"activejob\")\n\
         require \"active_support\"\nrequire \"active_job\"\nrequire \"logger\"\n\
         ActiveJob::Base.queue_adapter = :inline\nActiveJob::Base.logger = Logger.new(nil)\n\
         {program}"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .args(["-W0", "native.rb"])
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "native run failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

const EXPECTED_START: &str = r#"[:now, "done ok", ["prepended", "traced before ok", "traced around in", "own before", "own around in", "perform ok 2", "own around out", "after [:ok, 2]", "traced around out"]]
[:argument, ["ArgumentError", "bad"], ["prepended", "traced before argument", "traced around in", "own before", "own around in", "perform argument 1", "handled bad"]]
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn job_callbacks_and_handlers_run_like_activejob() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    for runtime in [
        "active_job_callbacks.rb",
        "active_support_rescuable.rb",
        "active_support_callbacks.rb",
    ] {
        assert!(dir.join("runtime").join(runtime).exists(), "{runtime}");
    }
    let facades =
        std::fs::read_to_string(dir.join("runtime/gem_facades.rb")).expect("gem_facades.rb");
    assert!(
        facades.contains("require_relative \"active_job_callbacks\""),
        "{facades}"
    );
    for (_, file, _) in FILES {
        let text =
            std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(!text.contains("roundhouse:"), "{file}: {text}");
    }
    let base = std::fs::read_to_string(dir.join("app/models/application_job.rb")).unwrap();
    assert!(base.contains("include ActiveJob::Callbacks"), "{base}");
    let report = std::fs::read_to_string(dir.join("app/models/report_job.rb")).unwrap();
    // The concern's callbacks at its include, the body's after it.
    let at = |needle: &str| {
        report
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {report}"))
    };
    assert!(
        at("include Traced") < at("before_enqueue :note_enqueue"),
        "{report}"
    );
    assert!(
        at("before_enqueue :note_enqueue") < at("include Stamped"),
        "{report}"
    );
    assert!(
        at("include Fallback") < at("before_perform :own_before"),
        "{report}"
    );
    assert!(!report.contains("def self.perform_later"), "{report}");
    let plain = std::fs::read_to_string(dir.join("app/models/plain_job.rb")).unwrap();
    assert!(
        plain.contains("def self.perform_later(value)")
            && plain.contains("def self.perform_now(value)"),
        "{plain}"
    );

    let script = format!(
        "require File.expand_path('main', Dir.pwd)\n{}{EMITTED_ADAPTER}{OBSERVE}",
        requires()
    );
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with(EXPECTED_START), "{stdout}");
    assert_eq!(stdout, native(), "emitted on the port vs native activejob");

    // The enqueue log the test helpers read counts the enqueue the chain
    // completes, not the one it refuses.
    let log = format!(
        "require File.expand_path('main', Dir.pwd)\n{}ActiveJob.enqueue_without_running\n\
         before = ActiveJob.performed.length\nReportJob.perform_later(:ok)\nReportJob.perform_later(:refuse)\nPlainJob.perform_later(1)\n\
         p ActiveJob.performed.drop(before), ActiveJob.perform_held([\"ReportJob\"]), ApplicationJob::TRAIL.include?(\"perform ok 1\")\n",
        requires()
    );
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &log])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout, "[\"ReportJob\", \"PlainJob\"]\n1\ntrue\n");
}

/// Spinel has no port and refuses the include by name.
#[test]
#[ignore = "fork spec: postgresql"]
fn spinel_refuses_the_callbacks_include() {
    let (dir, errors) = app().emit(BuildTarget::Spinel);
    assert!(!dir.join("runtime/active_job_callbacks.rb").exists());
    assert!(!dir.join("runtime/active_support_rescuable.rb").exists());
    let expected = "ApplicationJob includes ActiveJob::Callbacks, declared by the Ruby-family runtime (runtime/active_job_callbacks.rb), which spinel does not emit";
    assert!(errors.iter().any(|e| e.contains(expected)), "{errors:#?}");
}

/// An app whose jobs register no callback carries no port and keeps the
/// generated wrappers.
#[test]
fn jobs_without_callbacks_keep_their_wrappers() {
    let (dir, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/jobs/plain_job.rb", PLAIN_JOB)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(!dir.join("runtime/active_job_callbacks.rb").exists());
    let plain = std::fs::read_to_string(dir.join("app/models/plain_job.rb")).unwrap();
    assert!(
        !plain.contains("include ActiveJob::Callbacks")
            && plain.contains("def self.perform_later(value)"),
        "{plain}"
    );
}

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\nend\n";

const LATE_JOB: &str = r#"class LateJob < ApplicationJob
  include Stamped

  before_perform { |job| job.trail << "late own first" }
  around_perform do |job, block|
    job.trail << "late around in"
    block.call
    job.trail << "late around out"
  end
  rescue_from(StandardError) { |e| trail << "late rescued #{e.class}" }
  include Traced
  LIMIT = 2
  after_perform { |job| job.trail << "late after #{LIMIT}" }
  before_perform :late_own_last

  def perform(kind)
    trail << "late perform #{kind}"
    raise KeyError, "late" if kind == :key
    raise IOError, "late" if kind == :io
    kind
  end

  private

  def late_own_last
    trail << "late own last"
  end
end
"#;

const LATE_OBSERVE: &str = r#"def run(label)
  ApplicationJob::TRAIL.clear
  result = begin
    yield
  rescue Exception => e
    [:raised, e.class.name, e.message]
  end
  result = result.class.name if result.is_a?(ActiveJob::Base)
  p [label, result, ApplicationJob::TRAIL.dup]
end
run(:now) { LateJob.perform_now(:ok) }
run(:key) { LateJob.perform_now(:key) }
run(:io) { LateJob.perform_now(:io) }
run(:later) { LateJob.perform_later(:ok) }
p [LateJob._perform_callbacks.map(&:kind), LateJob.rescue_handlers.map(&:first), LateJob.stamp]
"#;

const LATE_FILES: &[(&str, &str, &str)] = &[
    ("app/jobs/application_job.rb", "app/models/application_job.rb", APPLICATION_JOB),
    ("app/jobs/concerns/traced.rb", "app/models/traced.rb", TRACED),
    ("app/jobs/concerns/stamped.rb", "app/models/stamped.rb", STAMPED),
    ("app/jobs/late_job.rb", "app/models/late_job.rb", LATE_JOB),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn an_include_below_an_own_callback_registers_where_it_stands() {
    let mut app = emit_and_run::empty_app().write("roundhouse.yml", "test_paths:\n  - test\n");
    for (path, _, text) in LATE_FILES {
        app = app.write(path, text);
    }
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let late = std::fs::read_to_string(dir.join("app/models/late_job.rb")).unwrap();
    assert!(!late.contains("roundhouse:"), "{late}");
    let at = |needle: &str| late.find(needle).unwrap_or_else(|| panic!("{needle}: {late}"));
    // The source's statement order: the include between the body's own.
    assert!(at("include Stamped") < at("late own first"), "{late}");
    assert!(at("rescue_from(StandardError)") < at("include Traced"), "{late}");
    assert!(at("include Traced") < at("before_enqueue :note_enqueue"), "{late}");
    assert!(at("before_enqueue :note_enqueue") < at("LIMIT = 2"), "{late}");
    assert!(at("LIMIT = 2") < at("before_perform :late_own_last"), "{late}");

    let requires: String = LATE_FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect();
    let script = format!("require File.expand_path('main', Dir.pwd)\n{requires}{LATE_OBSERVE}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let files: Vec<(&str, &str)> = LATE_FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    let native = native_of(&files, &format!("{requires}{LATE_OBSERVE}"));
    // The body's first callbacks run ahead of the concern's, and the
    // concern's KeyError handler, registered later, wins over the body's
    // StandardError one.
    assert!(
        native.starts_with(r#"[:now, :ok, ["late own first", "late around in", "traced before ok", "traced around in", "late own last", "late perform ok", "late after 2", "traced around out", "late around out"]]
[:key, #<KeyError: late>, ["late own first", "late around in", "traced before key", "traced around in", "late own last", "late perform key", "traced rescued late"]]
"#),
        "{native}"
    );
    assert_eq!(stdout, native, "emitted on the port vs native activejob");
}

const QUEUED: &str = r#"module Queued
  extend ActiveSupport::Concern

  included do
    queue_as :concerned
  end
end
"#;

const NAMELESS_JOB: &str = r#"class NamelessJob < ApplicationJob
  self.queue_name = self.default_queue_name = nil
end
"#;
const LOW_JOB: &str = r#"class LowJob < ApplicationJob
  queue_as :low
  before_enqueue { |job| job.trail << "enqueue on #{job.queue_name}" }

  def perform
    trail << "perform on #{queue_name}"
    queue_name
  end
end
"#;
const INHERITING_JOB: &str = r#"class InheritingJob < LowJob
end
"#;
const DYNAMIC_JOB: &str = r#"class DynamicJob < ApplicationJob
  queue_as do
    arguments.first == :paid ? :paid_feeds : :feeds
  end

  def perform(kind)
    queue_name
  end
end
"#;
const DEFAULT_JOB: &str = r#"class DefaultJob < ApplicationJob
  def perform
    queue_name
  end
end
"#;
const PREFIXED_JOB: &str = r#"class PrefixedJob < ApplicationJob
  self.queue_name_prefix = "shop"
  self.queue_name_delimiter = "."
  queue_as :mid

  def perform
    queue_name
  end
end
"#;
const CONCERNED_JOB: &str = r#"class ConcernedJob < ApplicationJob
  include Queued

  def perform
    queue_name
  end
end
"#;

const QUEUE_OBSERVE: &str = r#"p [LowJob.queue_name, InheritingJob.queue_name, PrefixedJob.queue_name, ConcernedJob.queue_name, NamelessJob.queue_name]
p [DefaultJob.queue_name.is_a?(Proc), DefaultJob.queue_name_delimiter, PrefixedJob.queue_name_delimiter, PrefixedJob.queue_name_prefix, LowJob.queue_name_prefix, PrefixedJob.queue_name_prefix?]
p [LowJob.perform_now, InheritingJob.perform_now, DynamicJob.perform_now(:paid), DynamicJob.perform_now(:free), DefaultJob.perform_now, PrefixedJob.perform_now, ConcernedJob.perform_now]
p [DynamicJob.new(:paid).queue_name, LowJob.new.queue_name_prefix, PrefixedJob.new.queue_name_prefix]
ApplicationJob::TRAIL.clear
LowJob.perform_later
InheritingJob.new.enqueue(queue: :other)
p ApplicationJob::TRAIL
p [LowJob.queue_name_from_part(:x), PrefixedJob.queue_name_from_part(nil), NamelessJob.default_queue_name]
LowJob.default_queue_name = :fallback
p [DefaultJob.perform_now, DefaultJob.default_queue_name, PrefixedJob.queue_name_from_part(nil)]
"#;

const QUEUE_FILES: &[(&str, &str, &str)] = &[
    ("app/jobs/application_job.rb", "app/models/application_job.rb", APPLICATION_JOB),
    ("app/jobs/concerns/queued.rb", "app/models/queued.rb", QUEUED),
    ("app/jobs/nameless_job.rb", "app/models/nameless_job.rb", NAMELESS_JOB),
    ("app/jobs/low_job.rb", "app/models/low_job.rb", LOW_JOB),
    ("app/jobs/inheriting_job.rb", "app/models/inheriting_job.rb", INHERITING_JOB),
    ("app/jobs/dynamic_job.rb", "app/models/dynamic_job.rb", DYNAMIC_JOB),
    ("app/jobs/default_job.rb", "app/models/default_job.rb", DEFAULT_JOB),
    ("app/jobs/prefixed_job.rb", "app/models/prefixed_job.rb", PREFIXED_JOB),
    ("app/jobs/concerned_job.rb", "app/models/concerned_job.rb", CONCERNED_JOB),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn queue_names_resolve_like_activejob() {
    let mut app = emit_and_run::empty_app().write("roundhouse.yml", "test_paths:\n  - test\n");
    for (path, _, text) in QUEUE_FILES {
        app = app.write(path, text);
    }
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let requires: String = QUEUE_FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect();
    let script = format!("require File.expand_path('main', Dir.pwd)\n{requires}{QUEUE_OBSERVE}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let files: Vec<(&str, &str)> = QUEUE_FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    let native = native_of(&files, &format!("{requires}{QUEUE_OBSERVE}"));
    assert!(
        native.starts_with(r#"["low", "low", "shop.mid", "concerned", nil]
"#),
        "{native}"
    );
    assert_eq!(stdout, native, "emitted on the port vs native activejob");
}

const DISCARDS: &str = r#"module Discards
  extend ActiveSupport::Concern

  RECORDED = []

  module ClassMethods
    def discard_on(*exceptions, report: false)
      RECORDED.concat(exceptions)
      super
    end
  end
end
"#;

const DISCARD_JOB: &str = r#"class DiscardJob < ApplicationJob
  include Discards

  queue_as :first
  after_discard { |job, error| job.trail << "after_discard #{error.class}" }
  discard_on KeyError
  discard_on(IOError) { |job, error| job.trail << "discarded #{error.message} #{job.arguments.inspect}" }
  rescue_from(RangeError) do |e|
    trail << "range #{executions} #{queue_name}"
    retry_job(queue: :again) if executions < 2
    trail << "kept #{queue_name}"
  end
  before_enqueue { |job| job.trail << "enqueue #{job.queue_name}" }

  def perform(kind)
    trail << "perform #{kind} #{executions}"
    raise KeyError, "k" if kind == :key
    raise IOError, "io" if kind == :io
    raise RangeError, "r" if kind == :range
    raise ArgumentError, "arg" if kind == :arg
    kind
  end
end
"#;

const DISCARD_OBSERVE: &str = r#"def run(label)
  ApplicationJob::TRAIL.clear
  result = begin
    yield
  rescue Exception => e
    [:raised, e.class.name, e.message]
  end
  result = result.class.name if result.is_a?(ActiveJob::Base)
  result = [result.class.name, result.message] if result.is_a?(Exception)
  p [label, result, ApplicationJob::TRAIL.dup]
end
run(:ok) { DiscardJob.perform_now(:ok) }
run(:key) { DiscardJob.perform_now(:key) }
run(:io) { DiscardJob.perform_now(:io) }
run(:arg) { DiscardJob.perform_now(:arg) }
run(:range) { DiscardJob.perform_later(:range) }
p [Discards::RECORDED, DiscardJob.rescue_handlers.map(&:first), DiscardJob.after_discard_procs.size, DiscardJob.retry_jitter]
"#;

const DISCARD_FILES: &[(&str, &str, &str)] = &[
    ("app/jobs/application_job.rb", "app/models/application_job.rb", APPLICATION_JOB),
    ("app/jobs/concerns/discards.rb", "app/models/discards.rb", DISCARDS),
    ("app/jobs/discard_job.rb", "app/models/discard_job.rb", DISCARD_JOB),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn discard_on_runs_like_activejob() {
    let mut app = emit_and_run::empty_app().write("roundhouse.yml", "test_paths:\n  - test\n");
    for (path, _, text) in DISCARD_FILES {
        app = app.write(path, text);
    }
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let requires: String = DISCARD_FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect();
    let script = format!("require File.expand_path('main', Dir.pwd)\n{requires}{DISCARD_OBSERVE}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let job = std::fs::read_to_string(dir.join("app/models/discard_job.rb")).unwrap();
    assert!(out.status.success(), "{stdout}\n{}\n{job}", String::from_utf8_lossy(&out.stderr));
    let files: Vec<(&str, &str)> = DISCARD_FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    let native = native_of(&files, &format!("{requires}{DISCARD_OBSERVE}"));
    assert!(native.contains("after_discard ArgumentError"), "{native}");
    assert_eq!(stdout, native, "emitted on the port vs native activejob");
}

/// The WithFallback hook is consumed for jobs only: a plain class that
/// includes it beside a class-attribute concern is still refused.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_registration_hook_is_consumed_for_jobs_only() {
    let widget = "class Widget\n  include Stamped\n  include Fallback\nend\n";
    let error = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/jobs/concerns/stamped.rb", STAMPED),
        ("app/jobs/fallback.rb", FALLBACK),
        ("app/models/widget.rb", widget),
    ]))
    .err()
    .expect("the hook is not consumed for a plain class")
    .to_string();
    assert!(
        error.contains("cannot be carried alongside an unconsumed included hook"),
        "{error}"
    );
}

const STAMP_JOB: &str = r#"class StampJob < ApplicationJob
  before_perform { |job| job.trail << "before" }

  def serialize
    super.merge("stamp" => 7)
  end

  def deserialize(job_data)
    super
    @stamp = job_data["stamp"]
  end

  def stamp
    @stamp
  end

  def perform(*args)
    trail << args
  end
end
"#;
const POINT: &str = r##"class Point
  attr_reader :x, :y

  def initialize(x, y)
    @x = x
    @y = y
  end

  def inspect
    "#<Point #{x},#{y}>"
  end
end
"##;
const POINT_SERIALIZER: &str = r#"class PointSerializer < ActiveJob::Serializers::ObjectSerializer
  def serialize(point)
    super("x" => point.x, "y" => point.y)
  end

  def deserialize(hash)
    Point.new(hash["x"], hash["y"])
  end

  def klass
    Point
  end
end
"#;
const SERIALIZE_OBSERVE: &str = r##"require "json"
ActiveJob::Serializers.add_serializers(PointSerializer)
job = StampJob.new(1, :two, { a: 3, "b" => [4.5, nil] }, 1...5, Time.utc(2026, 1, 2, 3, 4, 5), BigDecimal("1.5"), String, Point.new(1, 2), kw: true)
data = job.serialize
p data.reject { |k, _| %w[job_id enqueued_at locale timezone].include?(k) }
back = StampJob.deserialize(JSON.parse(JSON.generate(data)))
p [back.class, back.job_id == job.job_id, back.stamp, back.queue_name, back.executions]
p ActiveJob::Arguments.deserialize(data["arguments"])
[[Object.new], [{ 1 => 2 }], [{ "_aj_globalid" => 1 }]].each do |args|
  ActiveJob::Arguments.serialize(args)
rescue ActiveJob::SerializationError => e
  p e.message.sub(/#<Object:0x\h+>/, "#<Object>")
end
"##;
const SERIALIZE_FILES: &[(&str, &str, &str)] = &[
    ("app/jobs/application_job.rb", "app/models/application_job.rb", APPLICATION_JOB),
    ("app/models/point.rb", "app/models/point.rb", POINT),
    ("app/jobs/point_serializer.rb", "app/models/point_serializer.rb", POINT_SERIALIZER),
    ("app/jobs/stamp_job.rb", "app/models/stamp_job.rb", STAMP_JOB),
];

#[test]
#[ignore = "fork spec: postgresql layout"]
fn job_serialization_runs_like_activejob() {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_active_job_serialize\n");
    for (path, _, text) in SERIALIZE_FILES {
        app = app.write(path, text);
    }
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    // `check` names the PostgreSQL adapter as having no runtime of its
    // own in target-independent analysis; the CRuby tree carries one.
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let requires: String = SERIALIZE_FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect();
    for (_, file, _) in SERIALIZE_FILES {
        let text = std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(!text.contains("roundhouse:"), "{file}: {text}");
    }
    let script = format!(
        "require_relative 'runtime/active_job'\nrequire_relative 'runtime/active_job_errors'\n\
         require_relative 'runtime/active_job_callbacks'\n{requires}{SERIALIZE_OBSERVE}"
    );
    let out = emit_and_run::ruby().current_dir(&*dir).args(["-W0", "-e", &script]).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let files: Vec<(&str, &str)> = SERIALIZE_FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    let native = native_of(&files, &format!("{requires}{SERIALIZE_OBSERVE}"));
    assert!(native.contains("\"stamp\" => 7") && native.contains("PointSerializer"), "{native}");
    assert_eq!(stdout, native, "emitted on the port vs native activejob");
}

const CONFIGURED_JOB: &str = r#"class ConfiguredJob < ApplicationJob
  queue_as :plain
  before_enqueue do |job|
    at = job.scheduled_at ? (job.scheduled_at.to_f > Time.now.to_f + 30) : nil
    job.trail << "enqueue #{job.queue_name} #{job.priority.inspect} #{at.inspect} #{job.arguments.inspect}"
  end

  class << self
    def debounce(id)
      set(wait: 60, queue: :slow).perform_later(id, flag: true)
    end
  end

  def perform(*args)
    trail << "perform #{queue_name} #{priority.inspect} #{args.inspect}"
    args
  end
end
"#;

const CONFIGURED_OBSERVE: &str = r#"hold_jobs
def run(label)
  ApplicationJob::TRAIL.clear
  result = begin
    yield
  rescue Exception => e
    [:raised, e.class.name, e.message]
  end
  run_held
  result = result.class.name if result.is_a?(ActiveJob::Base)
  p [label, result, ApplicationJob::TRAIL.dup]
end
run(:plain) { ConfiguredJob.perform_later(1) }
run(:class_body) { ConfiguredJob.debounce(2) }
run(:queue) { ConfiguredJob.set(queue: :other).perform_later(3) }
run(:priority) { ConfiguredJob.set(priority: "7").perform_later(4) }
run(:wait_until) { ConfiguredJob.set(wait_until: Time.now + 3600).perform_later(5) }
run(:now) { ConfiguredJob.set(queue: :now_queue, priority: 2).perform_now(6) }
run(:none) { ConfiguredJob.set.perform_later }
p ConfiguredJob.set(wait: 1).class.name
"#;

const CONFIGURED_FILES: &[(&str, &str, &str)] = &[
    ("app/jobs/application_job.rb", "app/models/application_job.rb", APPLICATION_JOB),
    ("app/jobs/configured_job.rb", "app/models/configured_job.rb", CONFIGURED_JOB),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn set_answers_a_configured_job_like_activejob() {
    let mut app = emit_and_run::empty_app().write("roundhouse.yml", "test_paths:\n  - test\n");
    for (path, _, text) in CONFIGURED_FILES {
        app = app.write(path, text);
    }
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let requires: String = CONFIGURED_FILES
        .iter()
        .map(|(_, emitted, _)| format!("require_relative {:?}\n", emitted.trim_end_matches(".rb")))
        .collect();
    let script = format!("require File.expand_path('main', Dir.pwd)\n{requires}{EMITTED_ADAPTER}{CONFIGURED_OBSERVE}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", &script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let files: Vec<(&str, &str)> =
        CONFIGURED_FILES.iter().map(|(_, emitted, text)| (*emitted, *text)).collect();
    let native = native_of(&files, &format!("{requires}{NATIVE_ADAPTER}{CONFIGURED_OBSERVE}"));
    assert!(native.contains("enqueue slow nil true [2, {flag: true}]"), "{native}");
    assert!(native.contains("perform plain 7 [4]"), "{native}");
    assert!(native.contains("ActiveJob::ConfiguredJob"), "{native}");
    assert_eq!(stdout, native, "emitted on the port vs native activejob");
}
