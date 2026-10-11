//! `T.must(x)` as sorbet-runtime runs it, in emitted Ruby: the value
//! when it is not nil (`false` included), else `TypeError` with
//! sorbet's message ("Passed `nil` into T.must"), through
//! `T::Configuration`'s inline type error handler, whose default
//! re-raises. The argument is evaluated once, whether it is a local, an
//! ivar or a call. No tree loads sorbet-runtime: the check is lowered.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"class Lookup
  def initialize(names)
    @names = names
    @calls = 0
  end
  def first_name
    T.must(@names.first).upcase
  end
  def names
    T.must(@names)
  end
  def pass(value)
    T.must(value)
  end
  def counted
    T.must(bump)
    @calls
  end
  def bump
    @calls += 1
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
raise "names" unless lookup.names == ["ada", "grace"]
raise "zero" unless lookup.pass(0) == 0
raise "false" unless lookup.pass(false) == false
raise "evaluated once" unless lookup.counted == 1
fails_with("Passed `nil` into T.must") { Lookup.new([]).first_name }
fails_with("Passed `nil` into T.must") { Lookup.new([]).pass(nil) }
fails_with("Passed `nil` into T.must") { Lookup.new(nil).names }
puts "T.must contract passed"
"#;

#[test]
fn t_must_runs_in_emitted_ruby() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/lookup.rb", LIB)
        .run_ruby(SCRIPT)
        .assert_passes();
}
