//! The OpenTelemetry stub (`runtime/ruby/open_telemetry_facade.rb`) on
//! the Spinel target — the whole reason it exists outside
//! `GemFacade.fail!`'s raise-loudly contract: Spinel AOT can't link a
//! real `opentelemetry-api`/`-sdk`, so without this file
//! `OpenTelemetry::Trace.current_span` is an unresolved constant there,
//! not a raise `rescue StandardError` could even catch.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn open_telemetry_current_span_is_non_recording_under_spinel() {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n");
    let run = overlay.run_spinel(
        r#"
span = OpenTelemetry::Trace.current_span
raise "recording? must be false" if span.recording?
raise "set_attribute must return the span itself" unless span.set_attribute("k", "v").equal?(span)
puts "ok"
"#,
    );
    run.assert_passes();
}
