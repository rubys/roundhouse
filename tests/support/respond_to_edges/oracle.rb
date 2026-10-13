# ruby oracle.rb > expected.json  (actionpack + turbo-rails, same as ../turbo_respond_to/oracle.rb)
require "rails"
require "action_controller/railtie"
require "action_view/railtie"
require "turbo-rails"
require "jbuilder"
require "jbuilder/railtie"
require "rack/mock"
require "json"
HERE = __dir__
require File.join(HERE, "accepts")

ROUTES = {
  "h_side" => "probes", "h_side_json" => "probes", "h_multi" => "probes", "h_other" => "probes",
  "h_redirect" => "probes", "h_bare_noview" => "probes", "any_bare" => "probes", "any_multi" => "refused_probes",
  "mixed" => "probes", "force_json" => "refused_probes", "force_turbo" => "refused_probes",
  "force_plain" => "refused_probes", "pin" => "probes", "semi" => "probes", "h_cond" => "probes", "cond_arm" => "refused_probes", "helper_resp" => "refused_probes",
}
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
    ROUTES.each { |a, c| match "/probes/#{a}(.:format)", to: "#{c}##{a}", via: [:get, :post] }
  end
end
Rails.application.initialize!
class ApplicationController < ActionController::Base; end
ActionController::Base.prepend_view_path(File.join(HERE, "views"))
%w[probes_controller refused_probes_controller].each { |f| require File.join(HERE, f) }

# [action, accept-label-set]
EDGE_ACTIONS = %w[h_side h_multi h_bare_noview]
CORE_ACTIONS = ROUTES.keys
client = Rack::MockRequest.new(Rails.application)
rows = []
record = lambda do |name, path, label, extra = {}|
  env = extra.dup
  accept = ALL.fetch(label) if label
  env["HTTP_ACCEPT"] = accept if accept
  r = client.post(path, env)
  rows << { "name" => name, "path" => path, "accept_label" => label, "env" => extra, "status" => r.status,
            "content_type" => r.headers["content-type"], "location" => r.headers["location"], "body" => r.body }
end
CORE_ACTIONS.each { |a| CORE.each_key { |l| record.call("#{a} #{l}", "/probes/#{a}", l) } }
EDGE_ACTIONS.each { |a| EDGE.each_key { |l| record.call("#{a} #{l}", "/probes/#{a}", l) } }
# a few edge accepts against plain-like implicit actions
%w[force_plain].each { |a| EDGE.each_key { |l| record.call("#{a} #{l}", "/probes/#{a}", l) } }
%w[pin h_side_json].each do |a|
  %w[.json .turbo_stream .html].each { |ext| record.call("#{a} #{ext}", "/probes/#{a}#{ext}", "html") }
  %w[json turbo_stream html].each { |f| record.call("#{a} ?format=#{f}", "/probes/#{a}?format=#{f}", "html") }
end
%w[json html turbo].each { |l| record.call("cond_arm #{l}", "/probes/cond_arm", l); record.call("cond_arm j #{l}", "/probes/cond_arm?j=1", l) }
record.call("h_cond early", "/probes/h_cond?early=1", "turbo")
# XHR
%w[h_side h_multi].each do |a|
  record.call("#{a} xhr none", "/probes/#{a}", nil, "HTTP_X_REQUESTED_WITH" => "XMLHttpRequest")
  record.call("#{a} xhr starlow", "/probes/#{a}", "html_starlow", "HTTP_X_REQUESTED_WITH" => "XMLHttpRequest")
end
puts JSON.pretty_generate(rows)
