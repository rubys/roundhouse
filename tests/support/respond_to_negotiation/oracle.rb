require "rails"
require "action_controller/railtie"
require "action_view/railtie"
require "turbo-rails"
require "jbuilder"
require "jbuilder/railtie"
require "rack/mock"
require "json"
HERE = __dir__
require File.join(HERE, "reqs")
ACTIONS = %w[html_helper html_helper_redirect html_cond_redirect html_cond_head html_unless_render html_early_return html_rescue html_status pre_assign seq_if top_if wants_param multi_turbo three json_only html_only plain_two send_arm rescue_render shadow_local numbered it_param branch_diff top_branch_diff if_noelse seq_if_noelse else_other_stmt any_two nested_named nested_rest nested_it]
class OracleApp < Rails::Application
  config.root = HERE
  config.eager_load = false
  config.secret_key_base = "x" * 64
  config.logger = Logger.new(nil)
  config.hosts.clear
  config.consider_all_requests_local = false
  config.action_dispatch.show_exceptions = :rescuable
  config.action_dispatch.show_detailed_exceptions = false
  routes.append { ACTIONS.each { |a| match "/probes/#{a}(.:format)", to: "probes##{a}", via: [:get, :post] } }
end
Rails.application.initialize!
class ApplicationController < ActionController::Base; end
ActionController::Base.prepend_view_path(File.join(HERE, "views"))
require File.join(HERE, "probes_controller")
client = Rack::MockRequest.new(Rails.application)
rows = REQS.map do |name, path, accept, extra|
  env = extra.dup
  env["HTTP_ACCEPT"] = accept if accept
  begin
    r = client.post(path, env)
    { "name" => name, "status" => r.status, "content_type" => r.headers["content-type"], "location" => r.headers["location"], "body" => r.body }
  rescue Exception => e
    { "name" => name, "status" => -2, "content_type" => nil, "body" => "ORACLE RAISED #{e.class}" }
  end
end
puts JSON.pretty_generate(rows)
