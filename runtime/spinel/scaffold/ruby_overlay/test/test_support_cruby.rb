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

    def self.instrument(name, payload)
      return unless listening?(name)
      now = Time.now
      @subscribers.each do |pattern, callback|
        callback.call(name, now, now, nil, payload) if pattern === name
      end
    end
  end
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
