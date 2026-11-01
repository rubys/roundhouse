require "json"
require "stringio"

mode = ARGV.fetch(0)
if mode == "rails"
  require_relative "../rails_oracle"
  RailsOracle.activate("actionpack")
  require "action_controller"
  require_relative "concern"
  require_relative "controller"
  require_relative "other_controller"
  require_relative "custom_controller"
else
  require File.expand_path("main", Dir.pwd)
  Main.configure_default_adapter!
end

def controller_for(klass, rails)
  controller = klass.new
  controller.set_response!(ActionDispatch::Response.new) if rails
  controller
end

rails = mode == "rails"
rows = []
[
  ["plain", ["video/mp4"]],
  ["nil", ["video/mp4", nil]],
  ["initial_nil", [nil]],
  ["charset", ["text/plain; charset=ISO-8859-1"]],
  ["retained_charset", ["text/plain; charset=UTF-16", "application/json"]],
  ["symbol", [:json]],
  ["header_charset", ["application/json"]],
].each do |name, writes|
  begin
  controller = controller_for(MediaController, rails)
  controller.response.headers["Content-Type"] = "text/plain; charset=UTF-16" if name == "header_charset"
  results = writes.map { |value| controller.write_media(value: value) }
  controller.response_body = "body"
  if rails
    status, headers, body = controller.response.to_a
    chunks = body.each.to_a
    header = headers["content-type"]
  else
    status = controller.status
    header = controller.content_type
    chunks = [controller.body]
  end
  row = {case: name, results: results, media_type: controller.response.media_type,
           content_type: controller.response.content_type, final_header: header,
           status: status, body: chunks.join}
  if rails
    status, headers, body = MediaController.action(:show).call(Rack::MockRequest.env_for("/media?kind=#{name}"))
  else
    status, headers, body = Main.run_rack({"REQUEST_METHOD" => "GET", "PATH_INFO" => "/media",
      "QUERY_STRING" => "kind=#{name}", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new("")})
  end
  wire = {status: status, final_header: headers["content-type"], body: body.each.to_a.join}
  row[:wire] = wire
  if rails
    row[:cgi] = wire.merge(header_count: 1)
  else
    io = StringIO.new
    Main.run({"REQUEST_METHOD" => "GET", "PATH_INFO" => "/media", "QUERY_STRING" => "kind=#{name}",
      "HTTP_HOST" => "example.org"}, StringIO.new(""), io)
    head, payload = io.string.split("\r\n\r\n", 2)
    types = head.lines.filter_map { |line| line.split(":", 2)[1].strip if line.downcase.start_with?("content-type:") }
    row[:cgi] = {status: head[/Status: (\d+)/, 1].to_i, final_header: types.first,
                      header_count: types.length, body: payload}
  end
  rows << row
  rescue StandardError => error
    rows << {case: name, error_class: error.class.name}
  end
end

controller = controller_for(OtherMediaController, rails)
value = +"image/png"
result = controller.write_media(value: value)
raise "assigned value identity was lost" unless result.equal?(value)
rows << {case: "inherited_concern", result: result}

controller = controller_for(CustomMediaController, rails)
rows << {case: "override", result: controller.write_media(value: "image/png"),
         nil_result: controller.write_media(value: nil),
         method_result: controller.public_send(:content_type=, "image/png")}

puts JSON.generate(rows)
raise "Rails writer control failed" if rails && rows.any? { |row| row.key?(:error_class) }
