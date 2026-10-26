use super::emit_and_run;

fn api() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "/response/header", to: "responses#header"
  get "/response/render", to: "responses#rendered"
  get "/response/redirect", to: "responses#redirected"
  get "/response/head", to: "responses#empty"
  get "/response/symbol", to: "responses#symbolic"
end
"#,
        )
        .write(
            "app/controllers/responses_controller.rb",
            r#"class ResponsesController < ActionController::API
  after_action :write_probe_header, only: :header

  def header
    render plain: "header body"
  end

  def rendered
    code = params[:code].to_i
    render json: { ok: true }, status: code
  end

  def redirected
    code = params[:code].to_i
    redirect_to "/destination", status: code
  end

  def empty
    code = params[:code].to_i
    head code
  end

  def symbolic
    head :accepted
  end

  private

  def write_probe_header
    response.set_header("X-Probe", "first")
    value = response.set_header("X-Probe", "v")
    response.headers["X-Returned"] = value
  end
end
"#,
        )
}

#[test]
fn after_action_set_header_returns_its_value_and_reaches_the_wire() {
    api()
        .run_ruby(
            r#"
status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/response/header",
  "QUERY_STRING" => "", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "status #{status}" unless status == 200
raise "header #{headers.inspect}" unless headers["x-probe"] == "v"
raise "return value #{headers.inspect}" unless headers["x-returned"] == "v"
raise "body #{body.inspect}" unless body == ["header body"]
"#,
        )
        .assert_passes();
}

#[test]
fn computed_integer_statuses_reach_render_redirect_and_head_responses() {
    api()
        .run_ruby(
            r#"
[401, 404, 409, 422, 299].each do |code|
  status, _headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => "/response/render",
    "QUERY_STRING" => "code=#{code}", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  raise "render #{code}: #{status}" unless status == code
  raise "JSON body #{body.inspect}" unless JSON.parse(body.join) == { "ok" => true }
end

[301, 303, 307, 308].each do |code|
  status, headers, _body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => "/response/redirect",
    "QUERY_STRING" => "code=#{code}", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  raise "redirect #{code}: #{status}" unless status == code
  raise "location #{headers.inspect}" unless headers["location"] == "http://localhost/destination"
end

[204, 304].each do |code|
  status, headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => "/response/head",
    "QUERY_STRING" => "code=#{code}", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  raise "head #{code}: #{status}" unless status == code
  raise "bodyless content type #{headers.inspect}" if headers.key?("content-type")
  raise "head body #{body.inspect}" unless body == [""]
end

status, _headers, _body = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/response/symbol",
  "QUERY_STRING" => "", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "symbol head #{status}" unless status == 202

begin
  ActionController::Base.new.render("bad", status: :totally_invented)
  raise "unknown status was accepted"
rescue RuntimeError => error
  raise unless error.message.include?("Invalid HTTP status")
end
"#,
        )
        .assert_passes();
}
