# rails 8.1.4: a before_action target that the controller also calls: from
# the action, from a public helper, from a parameter default, and as a bound
# method (`method(:prepare)`, `&method(:prepare)`), and through `send` /
# `__send__` with a literal name or a computed one.
#
# Run it outside a Rails app's tree:
#
#     ruby tests/before_action_target_also_called/oracle.rb
require "rails"
require "active_record/railtie"
require "action_controller/railtie"
ENV["DATABASE_URL"] = "sqlite3::memory:"
class OracleApp < Rails::Application
  config.eager_load = false
  config.secret_key_base = "x" * 64
  config.logger = Logger.new(nil)
  config.hosts.clear
  config.action_dispatch.show_exceptions = :rescuable
end
OracleApp.initialize!
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define { create_table(:gadgets) { |t| t.string :name } }
OracleApp.routes.draw { resources :gadgets, only: :create }
class Gadget < ActiveRecord::Base; end
class GadgetsController < ActionController::Base
  skip_forgery_protection
  before_action :gadget_params, only: :create

  def create
    @gadget = Gadget.create!(name: gadget_params[:name])
    render json: { name: @gadget.name }
  end

  private

  def gadget_params
    params.require(:gadget).permit(:name)
  end
end
s = ActionDispatch::Integration::Session.new(OracleApp)
s.post "/gadgets", params: { gadget: { name: "z" } }
puts "with gadget: status=#{s.response.status} body=#{s.response.body}"
s.post "/gadgets", params: { other: 1 }
puts "without gadget: status=#{s.response.status}"
puts "gadgets: #{Gadget.count}"

s = ActionDispatch::Integration::Session.new(OracleApp)

# Each variant replaces GadgetsController; the router looks the constant up
# per request.
def variant(&body)
  Object.send(:remove_const, :GadgetsController)
  Object.const_set(:GadgetsController, Class.new(ActionController::Base))
  GadgetsController.class_eval do
    skip_forgery_protection
  end
  GadgetsController.class_eval(&body)
end

variants = {
  "public helper" => proc {
    before_action :gadget_params, only: :create
    def create
      @gadget = Gadget.create!(name: gadget_name)
      render json: { name: @gadget.name }
    end
    def gadget_name = gadget_params[:name]
    private
    def gadget_params = params.require(:gadget).permit(:name)
  },
  "parameter default" => proc {
    before_action :gadget_params, only: :create
    def create
      @gadget = Gadget.create!(name: gadget_name)
      render json: { name: @gadget.name }
    end
    private
    def gadget_name(attributes = gadget_params) = attributes[:name]
    def gadget_params = params.require(:gadget).permit(:name)
  },
  "method(:prepare)" => proc {
    before_action :prepare, only: :create
    def create
      @gadget = Gadget.create!(name: method(:prepare).call)
      render json: { name: @gadget.name }
    end
    private
    def prepare = @label = params[:gadget][:name]
  },
  "send(:prepare)" => proc {
    before_action :prepare, only: :create
    def create
      @gadget = Gadget.create!(name: send(:prepare))
      render json: { name: @gadget.name }
    end
    private
    def prepare = @label = params[:gadget][:name]
  },
  "__send__(\"prepare\")" => proc {
    before_action :prepare, only: :create
    def create
      @gadget = Gadget.create!(name: __send__("prepare"))
      render json: { name: @gadget.name }
    end
    private
    def prepare = @label = params[:gadget][:name]
  },
  "send(step), a computed name" => proc {
    before_action :prepare, only: :create
    def create
      step = "prep" + "are"
      @gadget = Gadget.create!(name: send(step))
      render json: { name: @gadget.name }
    end
    private
    def prepare = @label = params[:gadget][:name]
  },
  "&method(:prepare)" => proc {
    before_action :prepare, only: :create
    def create
      [params[:gadget][:name]].each(&method(:prepare))
      @gadget = Gadget.create!(name: @label)
      render json: { name: @gadget.name }
    end
    private
    def prepare(*) = @label = params[:gadget][:name]
  },
}
variants.each do |label, body|
  variant(&body)
  s.post "/gadgets", params: { gadget: { name: "z" } }
  puts "#{label}: status=#{s.response.status} body=#{s.response.body}"
end
