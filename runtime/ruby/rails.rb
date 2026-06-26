# frozen_string_literal: true

# Minimal `Rails` global for the roundhouse runtime. Covers the
# framework-level surface app code reaches *outside* the request path —
# `Rails.env` / `Rails.cache` / `Rails.logger` — plus an app-config
# stand-in (`Rails.application`).
#
# env/cache/logger are real (in-process) so the request path runs.
# `Rails.application`'s methods are defined in the source app's config,
# which roundhouse doesn't ingest yet, so they resolve to nil here rather
# than NameError-ing — a deliberate stub, not the app's real config.
module Rails
  def self.env
    @env ||= StringInquirer.new(ENV["RAILS_ENV"] || ENV["RACK_ENV"] || "development")
  end

  def self.cache
    @cache ||= Cache.new
  end

  def self.logger
    @logger ||= Logger.new
  end

  def self.application
    @application ||= Application.new
  end

  def self.root
    @root ||= Dir.pwd
  end

  # `Rails.env.production?` etc. — a String that answers `<value>?`.
  class StringInquirer < String
    def method_missing(name, *args)
      s = name.to_s
      s.end_with?("?") ? self == s[0..-2] : super
    end

    def respond_to_missing?(name, _include_private = false)
      name.to_s.end_with?("?") || super
    end
  end

  # In-process cache: `fetch` computes-and-stores on miss via the block.
  class Cache
    def initialize
      @store = {}
    end

    def fetch(key, _opts = {})
      return @store[key] if @store.key?(key)

      @store[key] = yield if block_given?
    end

    def read(key)
      @store[key]
    end

    def write(key, value, _opts = {})
      @store[key] = value
    end

    def delete(key)
      @store.delete(key)
    end
  end

  # No-op logger — the request path doesn't depend on log output.
  class Logger
    def info(*); end
    def error(*); end
    def warn(*); end
    def debug(*); end
    def fatal(*); end
  end

  # App-config stand-in (see module note). Unknown methods → nil.
  class Application
    def method_missing(*)
      nil
    end

    def respond_to_missing?(*)
      true
    end
  end
end
