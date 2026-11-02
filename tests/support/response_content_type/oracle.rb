# frozen_string_literal: true
require_relative "../rails_oracle"
exit 77 unless RailsOracle.available?("actionpack")
RailsOracle.activate("actionpack")
require "action_controller"
require "action_controller/api"
require "rack/mock"
require "json"
require_relative "controller"
rows = JSON.parse(File.read(File.join(__dir__, "expected.json"))).map do |name, _status, _type|
  action = name == "head" ? "head_ok" : name
  status, headers, body = ContentProbesController.action(action).call(Rack::MockRequest.env_for("http://example.org/" + name))
  body.close if body.respond_to?(:close)
  [name, status, headers["content-type"]]
end
puts JSON.generate(rows)
