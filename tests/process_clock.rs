//! `Process.clock_gettime(Process::CLOCK_*, unit)`: Ruby's monotonic,
//! wall and CPU clocks, as a test reporter reads them to time a run.
//! Every POSIX clock id Spinel's codegen lowers resolves and runs; an id
//! it does not lower stays an error rather than an unresolved read.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn clock_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("lib/clock.rb", r#"class Clock
  def monotonic_ms
    Process.clock_gettime(Process::CLOCK_MONOTONIC, :float_millisecond)
  end
  def monotonic_s
    Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end
  def realtime_ms
    Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
  end
  def cpu_ns
    Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID, :nanosecond)
  end
  def thread_cpu_ns
    Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID, :nanosecond)
  end
  def pid
    Process.pid
  end
end
"#)
        .write("test/clock_test.rb", r#"require "test_helper"
require "clock"
class ClockTest < ActiveSupport::TestCase
  test "monotonic" do
    clock = Clock.new
    a = clock.monotonic_ms
    b = clock.monotonic_ms
    assert_operator b, :>=, a
    assert_kind_of Float, a
    assert_kind_of Float, clock.monotonic_s
    assert_operator (clock.monotonic_ms - clock.monotonic_s * 1000.0).abs, :<, 1000.0
  end
  test "realtime and cpu" do
    clock = Clock.new
    assert_kind_of Integer, clock.realtime_ms
    assert_operator clock.realtime_ms, :>, 1_600_000_000_000
    assert_kind_of Integer, clock.cpu_ns
    assert_kind_of Integer, clock.thread_cpu_ns
    assert_kind_of Integer, clock.pid
    assert_operator clock.pid, :>, 0
  end
end
"#)
}

#[test]
fn process_clocks_run_in_emitted_ruby() {
    clock_app().run_test("test/models/clock_test.rb").assert_passes();
}

#[test]
fn an_unmodeled_process_clock_stays_an_error() {
    let (_, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/forker.rb", "class Forker\n  def clock\n    Process::CLOCK_BOOTTIME_ALARM\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("Process::CLOCK_BOOTTIME_ALARM")),
        "an unmodeled clock constant is not modeled: {errors:#?}"
    );
}
