#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SOURCE: &str = r#"class TimeoutProbe
  TIMEOUT_CLASS = IO::TimeoutError

  def self.timeout_message
    raise IO::TimeoutError, "read timed out"
  rescue IOError, IO::TimeoutError => error
    error.message
  end

  def self.unrelated_error
    raise ArgumentError, "invalid"
  rescue IO::TimeoutError
    "wrong"
  rescue ArgumentError
    "argument"
  end

  def self.rooted_error
    raise ::IO::TimeoutError, "rooted"
  rescue ::IO::TimeoutError => error
    error.message
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/services/timeout_probe.rb", SOURCE)
}

#[test]
fn io_timeout_error_executes_with_its_exception_identity() {
    app()
        .run_ruby(
            r#"
require_relative "app/models/timeout_probe"
raise "alias" unless TimeoutProbe::TIMEOUT_CLASS.equal?(IO::TimeoutError)
raise "parent" unless IO::TimeoutError < IOError
raise "message" unless TimeoutProbe.timeout_message == "read timed out"
raise "other exception" unless TimeoutProbe.unrelated_error == "argument"
raise "rooted" unless TimeoutProbe.rooted_error == "rooted"
puts "IO::TimeoutError keeps its class identity and rescue behavior"
"#,
        )
        .assert_passes();
}

#[test]
fn spinel_emission_keeps_io_timeout_error_for_the_runtime() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let source = std::fs::read_to_string(emitted.join("app/models/timeout_probe.rb")).unwrap();
    assert!(source.contains("IO::TimeoutError"), "{source}");
    assert!(
        !source.contains("roundhouse: constant not supported"),
        "{source}"
    );
}
