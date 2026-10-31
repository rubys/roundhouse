//! PostgreSQL: integration runner pg emit, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_integration_runner\n";
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const FILTER_TEST: &str = r#"require "test_helper"

class FilterTest < ActiveSupport::TestCase
  default_filter = "kept"
  test "reads the class local" do
    assert_equal "kept", @seen
  end

  setup do
    @seen = default_filter
  end
end
"#;

fn emit() -> emit_and_run::Emitted {
    let app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", "\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("test/models/filter_test.rb", FILTER_TEST);
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    emitted
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_runner_port_is_carried_and_loaded_with_the_test_cases() {
    let emitted = emit();
    assert!(emitted.join("runtime/action_dispatch_integration_runner.rb").exists());
    let hooks = std::fs::read_to_string(emitted.join("test/test_helper.rb")).unwrap();
    assert!(hooks.contains("action_dispatch_integration_runner"), "{hooks}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_setup_block_reading_a_class_local_is_defined_as_a_block_after_the_class_body() {
    let emitted = emit();
    let test = std::fs::read_to_string(emitted.join("test/models/filter_test.rb")).unwrap();
    let local = test.find("default_filter = \"kept\"").expect(&test);
    let block = test.find("define_method(:__roundhouse_before_setup) do").expect(&test);
    assert!(local < block, "{test}");
    assert!(!test.contains("def __roundhouse_before_setup"), "{test}");
    // The block's scope: the local is read where the class body wrote it.
    let script = r#"
class TestBase
  def __roundhouse_before_setup; end
end
class FilterTest < TestBase
end
class FilterTest
  default_filter = "kept"
  define_method(:__roundhouse_before_setup) do
    super()
    @seen = default_filter
  end
end
t = FilterTest.new
t.__roundhouse_before_setup
puts t.instance_variable_get(:@seen)
"#;
    let out = std::process::Command::new("ruby").arg("-e").arg(script).output().expect("ruby");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "kept");
}

/// The port against the small surface it prepends to: a request goes
/// through `process` (so a module extended onto the session sees it),
/// `header` rides every request and an explicit header wins, and the
/// recorder files an enqueue under `enqueued_jobs` or `performed_jobs`.
#[test]
#[ignore = "fork spec: postgresql"]
fn process_header_and_job_lists_behave_as_rails_does() {
    let emitted = emit();
    let runner = emitted.join("runtime/action_dispatch_integration_runner.rb");
    let script = format!(
        r##"
module RequestDispatch
  attr_reader :sent
  def get(path, params: {{}}, headers: {{}}, env: {{}}, xhr: false, as: nil)
    dispatch_request("GET", path, params, headers, as)
  end
  def post(path, params: {{}}, headers: {{}}, env: {{}}, xhr: false, as: nil) = dispatch_request("POST", path, params, headers, as)
  def patch(path, **) = nil
  def put(path, **) = nil
  def delete(path, **) = nil
  def head(path, **) = nil
  def dispatch_request(method, path, params, headers = {{}}, as = nil)
    (@sent ||= []) << [method, path, headers]
  end
  def reset!; end
end
module ActionDispatch
  class IntegrationTest; include RequestDispatch; end
end
class TestBase
  def name = "test_x"
end
module ActiveJob
  module TestHelper
    def clear_enqueued_jobs; :cleared; end
    def perform_enqueued_jobs(only: [], &block) = :performed
  end
  def self.enqueue_only = true
  module Enqueuing
    private
    def _raw_enqueue; self.successfully_enqueued = true; end
  end
end
class DemoJob
  include ActiveJob::Enqueuing
  attr_accessor :successfully_enqueued, :scheduled_at
  def successfully_enqueued? = successfully_enqueued
  def serialize = {{ "job_class" => "DemoJob", "arguments" => [1], "queue_name" => "default", "priority" => nil }}
  def enqueue = _raw_enqueue
end
class TestBase; include ActiveJob::TestHelper; end
require "{runner}"

lines = []
t = Class.new(ActionDispatch::IntegrationTest) {{
  def integration_session; super; end
}}.new
session = t.integration_session
lines << "session: #{{session.is_a?(RoundhouseIntegrationSession)}} #{{session.integration_session.equal?(session)}}"
wrapped = Module.new do
  def process(method, path, **options)
    super(method, path, **options.merge(headers: (options[:headers] || {{}}).merge("X-Edge" => "1")))
  end
end
session.extend(wrapped)
t.header("Accept", "application/json")
t.get("/a")
session.get("/b", headers: {{ "Accept" => "text/html" }})
lines.concat(t.sent.map {{ |m, p, h| "#{{m}} #{{p}} #{{h.sort.inspect}}" }})
t.header("Accept", nil)
t.reset!
lines << "method_name: #{{TestBase.new.method_name}}"

test = TestBase.new
Thread.current[:roundhouse_current_test] = test
DemoJob.new.enqueue
lines << "enqueued: #{{test.enqueued_jobs.map {{ |j| [j[:job], j[:args], j[:queue]] }}.inspect}}"
test.perform_enqueued_jobs
lines << "after perform: #{{test.enqueued_jobs.length}} #{{test.performed_jobs.length}}"
test.clear_enqueued_jobs
puts lines
"##,
        runner = runner.display()
    );
    let out = std::process::Command::new("ruby").arg("-e").arg(&script).output().expect("ruby");
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stderr), script);
    let got = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        got.lines().collect::<Vec<_>>(),
        [
            "session: true true",
            "GET /a [[\"Accept\", \"application/json\"], [\"X-Edge\", \"1\"]]",
            "GET /b [[\"Accept\", \"text/html\"], [\"X-Edge\", \"1\"]]",
            "method_name: test_x",
            "enqueued: [[DemoJob, [1], \"default\"]]",
            "after perform: 0 1",
        ],
        "{got}"
    );
}
