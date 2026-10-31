# frozen_string_literal: true
# rack-mini-profiler 4.0.0 for the Spinel tree, which cannot load the gem (MIT; LICENSE beside this file). The CRuby tree loads the gem itself.
require "securerandom"
require_relative "mini_profiler/config"
require_relative "mini_profiler/timer_struct"
require_relative "mini_profiler/file_store"
require_relative "mini_profiler/client_settings"

module Rack
  class MiniProfiler
    VERSION = "4.0.0"
    ASSET_VERSION = "e0bcc9ce0ae3bb5d6b736b6f282f601f"
    SOURCE_CODE_URI = "https://github.com/MiniProfiler/rack-mini-profiler"
    RESOURCES_ROOT = "runtime/gems/rack_mini_profiler/html"
    ID_ALPHABET = "0123456789abcdefghijklmnopqrstuvwxyz"
    CONFIG = Config.new

    def self.config
      CONFIG
    end

    # Not `rand(36**20).to_s(36)`: 36**20 overflows Int64 here, and twenty base-36 draws with the leading zeros dropped are the same distribution.
    def self.generate_id
      out = +""
      i = 0
      while i < 20
        d = rand(36)
        out << ID_ALPHABET[d] unless out.empty? && d == 0
        i += 1
      end
      out.empty? ? "0" : out
    end

    def self.authorize_request
      Thread.current[:mp_authorized] = true
    end

    def self.deauthorize_request
      Thread.current[:mp_authorized] = nil
    end

    def self.request_authorized?
      Thread.current[:mp_authorized]
    end

    def self.authorized?
      Thread.current[:mp_authorized] == true
    end
  end
end

require_relative "mini_profiler/request_hook"
require_relative "mini_profiler/railtie_methods"

Rack::MiniProfiler.config.apply_railtie(Rails.env)
RequestHooks.register(Rack::MiniProfiler::RequestHook.new)
Rack::MiniProfiler::RailtieMethods.subscribe
