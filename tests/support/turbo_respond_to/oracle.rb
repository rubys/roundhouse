# Regenerates expected.json: what actionpack 8.1.3.1 + turbo-rails 2.0.23
# answer for probes_controller.rb under each Accept header.
#
#   gem install rails turbo-rails
#   ruby tests/support/turbo_respond_to/oracle.rb > tests/support/turbo_respond_to/expected.json
#
# Each probe is POSTed (CSRF is skipped in the controller) so a turbo form
# submission's failure modes are the ones measured: a bare `format.turbo_stream`
# with no template, for instance, answers 204 to a POST but raises
# MissingExactTemplate to a browser GET (that case is refused by the lowering
# instead; see the test).
require "rails"
require "action_controller/railtie"
require "action_view/railtie"
require "turbo-rails"
require "rack/mock"
require "json"

HERE = __dir__

class OracleApp < Rails::Application
  config.root = HERE
  config.eager_load = false
  config.secret_key_base = "x" * 64
  config.logger = Logger.new(nil)
  config.hosts.clear
  config.consider_all_requests_local = false
  config.action_dispatch.show_exceptions = :rescuable
  config.action_dispatch.show_detailed_exceptions = false
  routes.append do
    PROBES.each { |a| match "/probes/#{a}", to: "probes##{a}", via: [:get, :post] }
  end
end
PROBES = %w[implicit inline many head_arm render_sym after_block turbo_only turbo_first html_json any_arm inherited html_bare]
Rails.application.initialize!
class ApplicationController < ActionController::Base; end
ActionController::Base.prepend_view_path(File.join(HERE, "views"))
require File.join(HERE, "probes_controller")

# label => Accept header (nil = none sent)
ACCEPTS = {
  "html"       => "text/html",
  "turbo"      => "text/vnd.turbo-stream.html, text/html, application/xhtml+xml",
  "json"       => "application/json",
  "star"       => "*/*",
  "none"       => nil,
  "html_turbo" => "text/html, text/vnd.turbo-stream.html",
  "json_html"  => "application/json, text/html",
  "png"        => "image/png",
  "browser"    => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
  "turbo_q"    => "text/html;q=0.5, text/vnd.turbo-stream.html"
}

client = Rack::MockRequest.new(Rails.application)
rows = []
record = lambda do |name, path, accept|
  env = {}
  env["HTTP_ACCEPT"] = accept if accept
  r = client.post(path, env)
  rows << { "name" => name, "path" => path, "accept" => accept, "status" => r.status,
            "content_type" => r.headers["content-type"], "body" => r.body }
end
PROBES.each { |a| ACCEPTS.each { |label, accept| record.call("#{a} #{label}", "/probes/#{a}", accept) } }
# A format PINNED by the URL beats Accept.
%w[implicit html_json].each do |a|
  %w[json turbo_stream].each { |ext| record.call("#{a} .#{ext}", "/probes/#{a}.#{ext}", "text/html") }
end
puts JSON.pretty_generate(rows)
