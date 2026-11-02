# What driver.rb and worker.rb share: the stand-ins for the app a tree
# on queue_adapter :sidekiq boots (Rails.application, the verifier, the
# database), and the runtime files tests/sidekiq_worker_live.rs copies
# beside them. Each defines its own JobRegistry after requiring this.
require "json"
require "securerandom"
require "time"
require_relative "base64"

module Rails
  class App
    def global_id_app
      "depot"
    end

    def secret_key_base
      ""
    end
  end

  def self.application
    App.new
  end
end

module ActionController
  class MessageVerifier
    def self.verified_data_json(secret, salt, data, purpose, rotate)
      ""
    end

    def self.json_value(json)
      ""
    end
  end
end

module Db
  def self.with_connection
    result = yield
    result
  end
end

# The worker clears the broadcast log (runtime/spinel/broadcasts.rb)
# before each job; these jobs broadcast nothing.
module Broadcasts
  def self.reset_log!
    nil
  end
end

require_relative "active_job"
require_relative "global_id_locator"
require_relative "active_job_serialization"
require_relative "sidekiq_redis"
require_relative "sidekiq_push"
require_relative "job_registry"
require_relative "sidekiq_envelope"
require_relative "sidekiq_worker"


# The app's `Sidekiq.configure_client` `url: ENV["REDIS_URL"]`, as the
# registry generates it: the worker has no environment fallback of its
# own.
module JobRegistry
  def self.sidekiq_redis_url
    SidekiqPush.setting("REDIS_URL")
  end
end
