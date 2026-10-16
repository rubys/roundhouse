//! `rescue_from` handlers see `exception.cause`, as Rails' do.
//!
//! `ActiveSupport::Rescuable#rescue_with_handler` tries the exception,
//! then its `cause`, then the cause's cause, and calls the first
//! handler it finds with the exception that matched. An error raised
//! inside a `rescue` gets the rescued one as its cause, so a handler for
//! the original error still answers. `rescue_from` was lowered to one
//! `rescue <Class>` clause per handler, so the wrapper escaped as a 500.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn rescue_from_handles_the_cause_of_an_unhandled_error() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            r##"class ApplicationController < ActionController::API
  class Missing < StandardError; end
  class Gone < StandardError; end
  rescue_from Missing, with: :missing
  rescue_from Gone, with: :gone

  private

  def gone(e)
    render json: { gone: e.message }, status: :gone
  end

  def missing(e)
    render json: { error: e.message }, status: :unprocessable_entity
  end
end
"##,
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def wrap
    raise Missing, "widget missing"
  rescue StandardError => e
    raise ArgumentError, "while handling: #{e.message}"
  end

  def deep
    begin
      raise Gone, "gone deep"
    rescue StandardError => e
      raise ArgumentError, "first: #{e.message}"
    end
  rescue ArgumentError => e
    raise RuntimeError, "second: #{e.message}"
  end

  def direct
    raise Missing, "direct"
  end

  def unhandled
    raise KeyError, "no handler"
  rescue StandardError => e
    raise ArgumentError, "wrapped: #{e.message}"
  end
end
"##,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"widgets/wrap\", to: \"widgets#wrap\"\n  post \"widgets/deep\", to: \"widgets#deep\"\n  post \"widgets/direct\", to: \"widgets#direct\"\n  post \"widgets/unhandled\", to: \"widgets#unhandled\"\nend\n",
        )
        .run_ruby(
            r##"
def probe(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => "",
                                         "CONTENT_TYPE" => "application/json", "rack.input" => StringIO.new("{}"))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
got = probe("/widgets/wrap")
raise "wrap #{got.inspect}" unless got == [422, %({"error":"widget missing"})]
got = probe("/widgets/deep")
raise "deep #{got.inspect}" unless got == [410, %({"gone":"gone deep"})]
got = probe("/widgets/direct")
raise "direct #{got.inspect}" unless got == [422, %({"error":"direct"})]
escaped = begin
  probe("/widgets/unhandled")
  nil
rescue ArgumentError => e
  e.message
end
raise "unhandled #{escaped.inspect}" unless escaped == "wrapped: no handler"
puts "ALL OK"
"##,
        )
        .assert_passes();
}
