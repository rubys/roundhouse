//! Rails 8.1.4 ActionController::Head must make arbitrary headers
//! observable on the emitted HTTP response, not only in HeaderStore.

use super::emit_and_run;

pub const ASSERTIONS: &str = r#"
status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "status #{status}" unless status == 201
raise "custom header #{headers.inspect}" unless headers["x-custom-header"] == "17"
raise "location #{headers.inspect}" unless headers["location"] == "/head-probe/created"
raise "content type #{headers.inspect}" unless headers["content-type"] == "text/plain"
raise "body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/json",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "symbol content type status #{status}" unless status == 200
raise "symbol content type #{headers.inspect}" unless headers["content-type"] == "application/json"
raise "symbol content type body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/negotiated.xml",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "negotiated status #{status}" unless status == 200
raise "negotiated XML #{headers.inspect}" unless headers["content-type"] == "application/xml"
raise "negotiated body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/empty",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "empty status #{status}" unless status == 204
raise "bodyless Content-Type #{headers.inspect}" if headers.key?("content-type")
raise "bodyless body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/reset",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "reset status #{status}" unless status == 205
raise "reset Content-Type #{headers.inspect}" if headers.key?("content-type")
raise "reset body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/not-modified",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "not-modified status #{status}" unless status == 304
raise "not-modified Content-Type #{headers.inspect}" if headers.key?("content-type")
raise "not-modified body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/defaulted",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "default status #{status}" unless status == 200
raise "default content type #{headers.inspect}" unless headers["content-type"] == "text/html"
raise "default body #{body.inspect}" unless body == [""]

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/model-location",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "model location status #{status}" unless status == 201
raise "model location #{headers.inspect}" unless headers["location"] == "/articles/1"
raise "model location body #{body.inspect}" unless body == [""]
puts "head Rack response passed"

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/head-probe/invalid-mime",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "invalid MIME recovery status #{status}" unless status == 202
raise "pre-failure status #{headers.inspect}" unless headers["x-original-status"] == "200"
raise "options mutation #{headers.inspect}" unless headers["x-options-preserved"] == "true"
raise "headers mutated #{headers.inspect}" unless headers["x-extra-headers-empty"] == "true"
raise "location mutated #{headers.inspect}" unless headers["x-location-empty"] == "true"
raise "invalid MIME body #{body.inspect}" unless body == [""]
puts "head invalid MIME state passed"
"#;

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/head-probe\", to: \"head_probes#created\"\n  get \"/head-probe/json\", to: \"head_probes#json\"\n  get \"/head-probe/negotiated\", to: \"head_probes#negotiated\"\n  get \"/head-probe/empty\", to: \"head_probes#empty\"\n  get \"/head-probe/reset\", to: \"head_probes#reset\"\n  get \"/head-probe/not-modified\", to: \"head_probes#not_modified\"\n  get \"/head-probe/defaulted\", to: \"head_probes#defaulted\"\n  get \"/head-probe/model-location\", to: \"head_probes#model_location\"\n  get \"/head-probe/invalid-mime\", to: \"head_probes#invalid_mime\"\n",
        )
        .write(
            "app/controllers/head_probes_controller.rb",
            r#"class HeadProbesController < ApplicationController
  def created
    head :created, { "x-custom_header" => 17, location: "/head-probe/created", content_type: "text/plain; charset=utf-8" }
  end

  def json
    head :ok, content_type: :json
  end

  def negotiated
    head :ok
  end

  def empty
    head :no_content
  end

  def reset
    head :reset_content
  end

  def not_modified
    head :not_modified
  end

  def defaulted
    head nil
  end

  def model_location
    @article = Article.create(title: "Head location", body: "A body long enough for validation.")
    head :created, location: @article
  end

  def invalid_mime
    options = { location: "/head-probe/unchanged", content_type: :unknown_head_mime, "x-custom" => "value" }
    begin
      head :created, options
    rescue ArgumentError
      head :accepted, {
        "x-original-status" => status.to_s,
        "x-options-preserved" => (options[:location] == "/head-probe/unchanged" && options[:content_type] == :unknown_head_mime && options["x-custom"] == "value").to_s,
        "x-extra-headers-empty" => (headers.size == 0).to_s,
        "x-location-empty" => location.nil?.to_s,
      }
    end
  end
end
"#,
        )
}

#[test]
fn head_options_are_emitted_in_the_rack_response() {
    let run = overlay().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("head Rack response passed"));
    assert!(run.stdout.contains("head invalid MIME state passed"));
}
