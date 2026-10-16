//! `T.must_because(x) { reason }` as sorbet-runtime runs it, in emitted
//! Ruby: the value when it is not nil (`false` included), else
//! `TypeError` with sorbet's message, `"Unexpected `nil` because "`
//! followed by the block's value. The reason is evaluated ONLY on the
//! nil path, where sorbet's own `yield` evaluates it, and the argument
//! is evaluated once. No tree loads sorbet-runtime: the check is
//! lowered.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"class Lookup
  def initialize(names)
    @names = names
    @calls = 0
    @reasons = 0
  end
  def first_name
    T.must_because(@names.first) { "the roster is never empty" }.upcase
  end
  def pass(value)
    T.must_because(value) { "callers filter nil" }
  end
  def counted
    T.must_because(bump) { "bump always answers" }
    @calls
  end
  def bump
    @calls += 1
  end
  def reasons
    @reasons
  end
  def lazy(value)
    T.must_because(value) { reason }
  end
  def reason
    @reasons += 1
    "reason ##{@reasons}"
  end
  def interpolated(value)
    T.must_because(value) { "roster held #{@names.length}" }
  end
end
"#;

const SCRIPT: &str = r#"
require_relative "app/models/lookup"
def fails_with(message)
  yield
  raise "expected TypeError"
rescue TypeError => error
  raise "wrong message: #{error.message}" unless error.message == message
end

lookup = Lookup.new(["ada", "grace"])
raise "first_name" unless lookup.first_name == "ADA"
raise "zero" unless lookup.pass(0) == 0
raise "false" unless lookup.pass(false) == false
raise "evaluated once" unless lookup.counted == 1

# The reason is not evaluated when the value is present.
raise "present" unless lookup.lazy("here") == "here"
raise "reason ran on the present path" unless lookup.reasons == 0

fails_with("Unexpected `nil` because the roster is never empty") { Lookup.new([]).first_name }
fails_with("Unexpected `nil` because callers filter nil") { lookup.pass(nil) }
fails_with("Unexpected `nil` because reason #1") { lookup.lazy(nil) }
raise "reason ran once on the nil path" unless lookup.reasons == 1
fails_with("Unexpected `nil` because roster held 2") { lookup.interpolated(nil) }
puts "T.must_because contract passed"
"#;

#[test]
fn t_must_because_runs_in_emitted_ruby() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/lookup.rb", LIB)
        .run_ruby(SCRIPT)
        .assert_passes();
}
