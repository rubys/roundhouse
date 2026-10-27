# Rails 8.1.4's own session store, as the oracle for the emitted app's:
# a `load_defaults 8.1` application with the session key and secret the
# contract in tests/support/encrypted_session_cookie.rs uses.
#
#   ruby rails_session_oracle.rb mint        # a session cookie Rails wrote
#   ruby rails_session_oracle.rb read RAW    # what Rails reads from RAW
#
# `mint` prints the cookie value a sign-in-like action left (an Integer,
# a String and a flash message in the session). `read` prints, as JSON,
# what a controller sees for that cookie: `session.to_hash`,
# `cookies.encrypted[session_options[:key]]` and `flash[:notice]`.
require "rails"
require "action_controller/railtie"
require "logger"
require "json"

SECRET = "roundhouse-encrypted-session-cookie-contract-secret"

class OracleApp < Rails::Application
  config.load_defaults 8.1
  config.secret_key_base = SECRET
  config.eager_load = false
  config.logger = Logger.new(nil)
  config.session_store :cookie_store, key: "_blog_session"
  config.hosts.clear
  routes.append do
    get "/mint" => "oracle#mint"
    get "/read" => "oracle#read"
  end
end

class OracleController < ActionController::Base
  def mint
    session[:authenticated_at] = 1760000000
    session[:user] = "grace"
    flash[:notice] = "From Rails"
    head :ok
  end

  def read
    seen = {
      "session" => session.to_hash,
      "encrypted" => cookies.encrypted[Rails.application.config.session_options[:key]],
      "notice" => flash[:notice],
    }
    render plain: JSON.generate(seen)
  end
end

OracleApp.initialize!
mock = Rack::MockRequest.new(OracleApp)
case ARGV[0]
when "mint"
  header = mock.get("/mint", "HTTP_HOST" => "blog.test").headers["set-cookie"].to_s
  puts Rack::Utils.unescape(header[/_blog_session=([^;]+)/, 1].to_s)
when "read"
  cookie = "_blog_session=" + Rack::Utils.escape(ARGV[1].to_s)
  puts mock.get("/read", "HTTP_HOST" => "blog.test", "HTTP_COOKIE" => cookie).body
else
  abort "usage: rails_session_oracle.rb mint | read RAW"
end
