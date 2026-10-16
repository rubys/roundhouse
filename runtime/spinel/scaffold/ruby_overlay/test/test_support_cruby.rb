# frozen_string_literal: true
# Test support the CRuby tree can give and the spinel tree cannot:
# loaded last by test/test_helper.rb on the ruby target only
# (`project.rs::apply_cruby_test_support`).
#
# Rails' `ActiveSupport::Testing::ConstantStubbing#stub_const`, as Rails
# writes it: the constant is REPLACED for the block and put back after,
# so code that reads it at call time (`Message::Pagination::PAGE_SIZE`,
# `Opengraph::Fetch::TIMEOUT`) sees the stub. A compiled tree resolves
# constants at build time and has no `const_set`; there the helper is
# absent and the call fails as the gap it is.
class TestBase
  def stub_const(mod, constant, new_value)
    old_value = mod.const_get(constant, false)
    begin
      mod.send(:remove_const, constant)
      mod.const_set(constant, new_value)
      yield
    ensure
      mod.send(:remove_const, constant) if mod.const_defined?(constant, false)
      mod.const_set(constant, old_value)
    end
  end
end

# Rails-dom-testing's `css_select`: the elements of the last response
# matching `selector`, as Nokogiri nodes (`node["id"]`). Rails' own is
# Nokogiri; the shared harness's `Dom` is a scanner that matches a
# selector's LAST compound and keeps no per-element attributes, so a
# descendant selector read from it would answer for the whole page.
require "nokogiri"

class TestBase
  def css_select(selector)
    Nokogiri::HTML5.fragment(@__response.body.to_s).css(selector).to_a
  end
end

# ActionCable::TestHelper#assert_broadcast_on: some broadcast to
# `stream` — during the block, or so far in this test without one —
# carries `data`, compared as Rails compares it, after a JSON round
# trip (symbol keys and all become what the wire carries).
module ActionCable
  module TestHelper
    def assert_broadcast_on(stream, data, &block)
      entries =
        if block
          capture_broadcasts_on(stream, &block)
        else
          Broadcasts.log.select { |entry| entry[:stream] == stream }
        end
      # The CRuby tree logs the Hash it publishes, the spinel tree the
      # JSON text; both compare after the same round trip.
      wire = ->(value) { ::JSON.parse(value.is_a?(String) ? value : ::JSON.generate(value)) }
      want = wire.(data)
      payloads = entries.map { |entry| entry[:payload] }.compact
      return if payloads.any? { |payload| wire.(payload) == want }
      raise "assert_broadcast_on failed: no broadcast to #{stream.inspect} carried #{want.inspect}; " \
            "got #{payloads.inspect}"
    end
  end
end

# `ActiveSupport::Notifications.subscribe(pattern) { |name, start,
# finish, id, payload| … }` / `unsubscribe(subscriber)` for the cache
# events, `cache_read` / `cache_write.active_support` with the key in
# the payload, which campfire's caching test reads to learn which
# fragments were cached. The store's typed seam (`read_str` /
# `write_str`, every view `cache` and `cached: true` render) reports
# them while anyone listens. The keys are Rails' shape
# (`views/messages/_message/messages/4-…`), template path included.
# A pattern is a String (the exact name) or a Regexp, as in Rails.
# The spinel tree has no subscriber list; there the call is the gap.
module ActiveSupport
  module Notifications
    @subscribers = []

    def self.subscribe(pattern, callback = nil, &block)
      subscriber = [pattern, callback || block]
      @subscribers << subscriber
      subscriber
    end

    def self.unsubscribe(subscriber)
      @subscribers.delete(subscriber)
      nil
    end

    def self.listening?(name)
      @subscribers.any? { |pattern, _| pattern === name }
    end

    # A one-argument callback gets an `Event`, as Rails hands one to a
    # subscriber whose block or callable takes one (`->(event) {
    # event.payload[:hits] }`); any other arity gets the five arguments.
    def self.instrument(name, payload)
      return unless listening?(name)
      now = Time.now
      @subscribers.dup.each do |pattern, callback|
        next unless pattern === name
        if callback.arity == 1
          callback.call(Event.new(name, now, now, nil, payload))
        else
          callback.call(name, now, now, nil, payload)
        end
      end
    end

    class Event
      attr_reader :name, :time, :end, :transaction_id, :payload

      def initialize(name, start, ending, transaction_id, payload)
        @name = name
        @time = start
        @end = ending
        @transaction_id = transaction_id
        @payload = payload
      end
    end

    # `subscribed(callback, name) { … }` for every event this file
    # reports, not just SQL: subscribed for the block, unsubscribed
    # after, as Rails does. SQL keeps the shared harness's capture,
    # which reads every statement and is not an instrument call here.
    class << self
      alias_method :__rh_shared_subscribed, :subscribed

      def subscribed(callback, name, &block)
        return __rh_shared_subscribed(callback, name, &block) if name == "sql.active_record"
        subscriber = subscribe(name, callback)
        begin
          block.call
        ensure
          unsubscribe(subscriber)
        end
      end
    end
  end
end

# ActiveSupport's own store reports what Rails' does: `cache_read` with
# the normalized key and `hit`, `cache_write`, and `cache_read_multi`
# with the keys asked for and the ones found (`hits`) — the events
# campfire's fragment tests subscribe to. A fetch is a read, then on a
# miss a write, as Rails instruments it.
module ActiveSupport
  module Cache
    class MemoryStore
      module Instrumented
        def read(name, options = nil)
          key = ActiveSupport::Cache.expanded_key(name)
          hit = exist?(name)
          value = super
          ActiveSupport::Notifications.instrument("cache_read.active_support", { key: key, hit: hit })
          value
        end

        def write(name, value, options = nil)
          stored = super
          ActiveSupport::Notifications.instrument("cache_write.active_support", { key: ActiveSupport::Cache.expanded_key(name) })
          stored
        end

        def fetch(name)
          key = ActiveSupport::Cache.expanded_key(name)
          hit = exist?(name)
          ActiveSupport::Notifications.instrument("cache_read.active_support", { key: key, hit: hit, super_operation: :fetch })
          super
        end

        def read_multi(*names)
          found = super
          ActiveSupport::Notifications.instrument("cache_read_multi.active_support", {
            key: names.map { |name| ActiveSupport::Cache.expanded_key(name) },
            hits: found.keys.map { |name| ActiveSupport::Cache.expanded_key(name) }
          })
          found
        end
      end
      prepend Instrumented
    end
  end
end

# Rails' collection renderer reports `render_collection.action_view`
# once per collection: the partial's template file, the element count,
# and — under collection caching — how many fragments were served from
# the store. The runtime's hook carries the facts; this reports them.
module ActionView
  module ViewHelpers
    class << self
      alias_method :__rh_collection_rendered, :collection_rendered

      def collection_rendered(identifier, hits)
        ActiveSupport::Notifications.instrument("render_collection.action_view", {
          identifier: File.join(Dir.pwd, identifier),
          count: hits.length,
          cache_hits: hits.count { |hit| !hit.nil? }
        })
        __rh_collection_rendered(identifier, hits)
      end
    end
  end
end

# The integration session's cookie jar is Rack::Test's, which answers nil
# for a cookie the browser never held — `assert_not cookies[
# "_campfire_session"]` asks exactly that — and "" for one a response
# deleted, as it keeps the expired entry (Rails 8's generated
# `assert_empty cookies[:session_id]` after sign-out reads it). The
# shared harness drops a deleted cookie (`accept_cookies`), so the names
# it dropped are remembered here; the runtime's controller-side jar
# answers "" for both (typed String, see action_controller/cookies.rb).
# The names live on the test, not the jar: the harness builds a fresh jar
# from each response's accepted cookies.
module TestCookieReads
  attr_writer :deleted_cookie_names

  def [](key)
    name = key.to_s
    return super if to_h.key?(name)
    (@deleted_cookie_names || []).include?(name) ? "" : nil
  end
end

module RequestDispatch
  module NilCookieReads
    def cookies
      jar = super
      jar.extend(TestCookieReads) unless jar.is_a?(TestCookieReads)
      jar.deleted_cookie_names = deleted_cookie_names
      jar
    end

    def deleted_cookie_names
      @__deleted_cookie_names ||= []
    end

    def accept_cookies(carried, pending)
      names = deleted_cookie_names
      pending.each do |k, v|
        if v == ""
          names << k unless names.include?(k)
        else
          names.delete(k)
        end
      end
      super
    end
  end
  prepend NilCookieReads
end

# rails-dom-testing's `assert_select` hands its block the matched
# elements; the shared harness's string scanner has no elements to
# hand. Here they are Nokogiri's, selected from the same root.
module RequestDispatch
  module SelectedElements
    def assert_select(selector_or_root, content_or_opts = nil, opts = nil, &block)
      return super unless block
      super(selector_or_root, content_or_opts, opts, &nil)
      if selector_or_root.is_a?(String)
        root = Nokogiri::HTML5.fragment(@__response.body.to_s)
        selector = selector_or_root
        substitute = content_or_opts
      else
        root = selector_or_root
        root = Nokogiri::HTML5.fragment(root.to_s) if root.is_a?(String)
        selector = content_or_opts
        substitute = opts
      end
      if selector.include?("?") && (substitute.is_a?(String) || substitute.is_a?(Numeric))
        selector = selector.sub("?", substitute.to_s.inspect)
      end
      block.call(root.css(selector))
    end
  end
  prepend SelectedElements
end

module Rails
  class MemoryStore
    module Instrumented
      def read_str(key)
        value = super
        ActiveSupport::Notifications.instrument("cache_read.active_support", { key: key.to_s, hit: !value.nil? })
        value
      end

      def write_str(key, value, ttl)
        stored = super
        ActiveSupport::Notifications.instrument("cache_write.active_support", { key: key.to_s })
        stored
      end
    end
    prepend Instrumented
  end
end

# Rails' time helpers stub `Time.now`: under `travel_to` / `freeze_time`
# app and test code that asks the stdlib clock — `Time.current`, which
# the lowering grounds to `Time.now.utc`, included — reads the traveled
# instant, the same one `ActiveSupport.now` (and so every stamped
# `created_at`) reads. The harness's helpers move `ActiveSupport`'s
# clock; this lets `Time.now` follow it while one is set, and reads
# the real clock otherwise. A compiled tree cannot redefine `Time.now`;
# there the test clock reaches only what goes through `ActiveSupport`.
class << Time
  alias_method :__roundhouse_real_now, :now

  def now(**options)
    traveling = ActiveSupport::FROZEN_AT[0] != 0 || ActiveSupport::TRAVEL_OFFSET[0] != 0
    return __roundhouse_real_now(**options) unless traveling && options.empty?
    ActiveSupport.clock
  end
end

module ActiveSupport
  def self.clock
    frozen = FROZEN_AT[0]
    return Time.at(frozen) if frozen != 0
    Time.__roundhouse_real_now + TRAVEL_OFFSET[0]
  end
end

class TestBase
  # Relative to the REAL clock, which is what the offset is kept against;
  # `Time.now` may already be traveled.
  def travel_to(target)
    ActiveSupport.travel(target.to_i - Time.__roundhouse_real_now.to_i)
    return unless block_given?
    begin
      yield
    ensure
      travel_back
    end
  end
end
