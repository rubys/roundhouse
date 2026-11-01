//! API callback/rescue responses compared with Rails and emitted Ruby.
use super::emit_and_run;

pub const SOURCES: &[(&str, &str)] = &[
    (
        "app/controllers/application_controller.rb",
        r#"
class ApplicationController < ActionController::API
  rescue_from KeyError do |failure|
    render json: { owner: "parent", errors: [failure.message] }, status: :unprocessable_entity
  end
  after_action { response.set_header("X-After", "yes") }
end
"#,
    ),
    (
        "app/controllers/concerns/json_guard.rb",
        r#"
module JsonGuard
  extend ActiveSupport::Concern
  included do
    before_action do
      render json: { owner: "concern" }, status: :unauthorized if params[:deny] == "1"
    end
  end
end
"#,
    ),
    (
        "app/controllers/probes_controller.rb",
        r#"
class ProbesController < ApplicationController
  before_action only: :index, if: -> { params[:deny] == "1" } do
    payload = { owner: "callback", title: params[:item][:title] }
    copied = payload
    render json: copied, status: :forbidden
  end
  before_action only: :index, if: -> { params[:deny] == "1" } do
    raise "callback chain did not halt"
  end
  rescue_from ArgumentError do |failure|
    render json: { owner: "earlier", errors: [failure.message] }, status: :bad_request
  end
  rescue_from ArgumentError do |failure|
    payload = { owner: "child", errors: [failure.message], limit: params.fetch(:limit, "none") }
    render json: payload, status: :conflict
  end
  def index
    render json: { owner: "action", title: params[:item][:title] }, status: :accepted
  end
  def show
    raise ArgumentError, "boom"
  end
  def fallback
    raise KeyError, "fallback"
  end
  def quiet
  end
end
"#,
    ),
    (
        "app/controllers/descendants_controller.rb",
        r#"
class DescendantsController < ProbesController
  def show
    raise ArgumentError, "descendant"
  end
end
"#,
    ),
    (
        "app/controllers/inherited_controller.rb",
        r#"
class InheritedController < ApplicationController
  def show
    raise KeyError, "inherited"
  end
end
"#,
    ),
    (
        "app/controllers/guarded_controller.rb",
        r#"
class GuardedController < ApplicationController
  include JsonGuard
  def index
    render json: { owner: "concern-action" }
  end
end
"#,
    ),
    (
        "app/controllers/passives_controller.rb",
        r#"
class PassivesController < ApplicationController
  before_action { response.set_header("X-Before", "yes") }
  def index
    render json: { owner: "passive" }, status: :created
  end
end
"#,
    ),
    (
        "app/controllers/named_controller.rb",
        r#"
class NamedController < ActionController::API
  before_action :deny
  def index
    raise "named callback did not halt"
  end
  private
  def deny
    render json: { owner: "named" }, status: :bad_request
  end
end
"#,
    ),
    (
        "config/routes.rb",
        r#"
Rails.application.routes.draw do
  get "/probes", to: "probes#index"
  get "/child", to: "probes#show"
  get "/fallback", to: "probes#fallback"
  get "/quiet", to: "probes#quiet"
  get "/descendant", to: "descendants#show"
  get "/inherited", to: "inherited#show"
  get "/guarded", to: "guarded#index"
  get "/passive", to: "passives#index"
  get "/named", to: "named#index"
end
"#,
    ),
];

pub fn app() -> emit_and_run::Overlay {
    SOURCES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, source)| {
            app.write(path, source)
        })
}

// Also used verbatim by the Rails oracle. It observes JSON values rather
// than encoder whitespace, and headers only where callbacks write them.
pub const PROGRAM: &str = r#"
require "json"
if defined?(Main)
  require File.expand_path("app/controllers/probes_controller", Dir.pwd)
  require File.expand_path("app/controllers/descendants_controller", Dir.pwd)
  require File.expand_path("app/controllers/inherited_controller", Dir.pwd)
  require File.expand_path("app/controllers/guarded_controller", Dir.pwd)
  require File.expand_path("app/controllers/passives_controller", Dir.pwd)
  require File.expand_path("app/controllers/named_controller", Dir.pwd)
end
def observe(label, klass, action, path)
  if defined?(Main)
    uri, query = path.split("?", 2)
    status, headers, response = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => uri,
      "QUERY_STRING" => query.to_s, "rack.input" => StringIO.new(""))
  else
    status, headers, response = klass.action(action).call(Rack::MockRequest.env_for(path))
  end
  text = +""
  response.each { |part| text << part }
  response.close if response.respond_to?(:close)
  body = text.empty? ? "" : JSON.parse(text)
  after = headers["x-after"] || headers["X-After"]
  before = headers["x-before"] || headers["X-Before"]
  puts JSON.generate([label, status, body, after, before])
end
observe("blocked", ProbesController, :index, "/probes?deny=1&item[title]=denied")
observe("allowed", ProbesController, :index, "/probes?item[title]=allowed")
observe("child", ProbesController, :show, "/child?limit=3")
observe("descendant", DescendantsController, :show, "/descendant?limit=4")
observe("inherited", InheritedController, :show, "/inherited")
observe("fallback", ProbesController, :fallback, "/fallback")
observe("concern-blocked", GuardedController, :index, "/guarded?deny=1")
observe("concern-allowed", GuardedController, :index, "/guarded")
observe("passive", PassivesController, :index, "/passive")
observe("quiet", ProbesController, :quiet, "/quiet")
observe("named", NamedController, :index, "/named")
"#;

pub const EXPECTED: &str = r#"["blocked",403,{"owner":"callback","title":"denied"},null,null]
["allowed",202,{"owner":"action","title":"allowed"},"yes",null]
["child",409,{"owner":"child","errors":["boom"],"limit":"3"},null,null]
["descendant",409,{"owner":"child","errors":["descendant"],"limit":"4"},null,null]
["inherited",422,{"owner":"parent","errors":["inherited"]},null,null]
["fallback",422,{"owner":"parent","errors":["fallback"]},null,null]
["concern-blocked",401,{"owner":"concern"},null,null]
["concern-allowed",200,{"owner":"concern-action"},"yes",null]
["passive",201,{"owner":"passive"},"yes","yes"]
["quiet",204,"","yes",null]
["named",400,{"owner":"named"},null,null]"#;
