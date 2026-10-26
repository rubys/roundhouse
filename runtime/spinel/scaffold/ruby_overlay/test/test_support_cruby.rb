# frozen_string_literal: true
# Test support the CRuby tree can give and the spinel tree cannot:
# loaded last by test/test_helper.rb on the ruby target only
# (`project.rs::apply_cruby_test_support`).
#
# Rails' `ActiveSupport::Testing::ConstantStubbing#stub_const`, as Rails
# writes it: the constant is REPLACED for the block and put back after,
# so code that reads it at call time (`Message::Pagination::PAGE_SIZE`,
# `Opengraph::Fetch::TIMEOUT`) sees the stub. Most sites never reach
# this: `lower::stub_const` rewrites them to a value stack every target
# shares. What is left is a stub that changes the constant's type, or
# one read reflectively (`const_get`) — here the real helper answers
# them; the spinel harness's fails the test saying why.
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
# A spinel tree that carries spinel-nokogiri gets the same helper
# (`project::SPINEL_CSS_SELECT`).
require "nokogiri"

# rails-dom-testing parses with HTML5 where Nokogiri has the parser and
# HTML4 where it does not, which is JRuby.
HTML_FRAGMENT_PARSER = defined?(Nokogiri::HTML5) ? Nokogiri::HTML5 : Nokogiri::HTML4

class TestBase
  def css_select(selector)
    HTML_FRAGMENT_PARSER.fragment(@__response.body.to_s).css(selector).to_a
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
      # The CRuby tree logs the Hash it publishes; the shared harness's
      # version, which this replaces, reads the spinel tree's JSON text.
      # Both compare after the same round trip.
      wire = ->(value) { ::JSON.parse(value.is_a?(String) ? value : ::JSON.generate(value)) }
      want = wire.(data)
      payloads = entries.map { |entry| entry[:payload] }.compact
      return if payloads.any? { |payload| wire.(payload) == want }
      raise "assert_broadcast_on failed: no broadcast to #{stream.inspect} carried #{want.inspect}; " \
            "got #{payloads.inspect}"
    end
  end
end

# Cache events go through `ActiveSupport::Notifications` in
# runtime/ruby/active_support_notifications.rb. This overlay used to
# keep a second subscriber list; the store below only publishes.

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
        root = HTML_FRAGMENT_PARSER.fragment(@__response.body.to_s)
        selector = selector_or_root
        substitute = content_or_opts
      else
        root = selector_or_root
        root = HTML_FRAGMENT_PARSER.fragment(root.to_s) if root.is_a?(String)
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
