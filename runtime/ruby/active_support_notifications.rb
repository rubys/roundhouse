# frozen_string_literal: true
# `ActiveSupport::Notifications`. Subscribe for the block, unsubscribe
# after. A one-argument callback receives an Event; any other arity
# receives the five arguments Rails passes. `monotonic:` is accepted
# and ignored: this runtime has one clock.
#
# `Db.record_query` calls `sql`, so a `subscribe("sql.active_record")`
# hears each statement. `subscribed` for that name still drains
# `Db.capture_sql` after the block. The payload carries `:sql` and
# `:name` (`"SQL"`).
module ActiveSupport
  module Notifications
    SUBSCRIBERS = []

    def self.subscribe(pattern, callback = nil, monotonic: false, &block)
      _ = monotonic
      listener = callback.nil? ? block : callback
      subscriber = [pattern, listener]
      SUBSCRIBERS << subscriber
      subscriber
    end

    def self.unsubscribe(subscriber)
      SUBSCRIBERS.delete(subscriber)
      nil
    end

    def self.instrument(name, payload = nil, &block)
      payload = {} if payload.nil?
      start = Time.now
      start_f = start.to_f
      value = nil
      begin
        value = block.nil? ? nil : yield
      ensure
        finish = Time.now
        span = finish.to_f - start_f
        i = 0
        while i < SUBSCRIBERS.length
          entry = SUBSCRIBERS[i]
          pattern = entry[0]
          callback = entry[1]
          if pattern === name
            if callback.arity == 1
              callback.call(Event.new(name, start, finish, nil, payload, span))
            else
              callback.call(name, start, finish, nil, payload)
            end
          end
          i += 1
        end
      end
      value
    end

    # One statement from `Db.record_query`. A subscriber that itself
    # queries does not re-enter, or the callback would notify itself.
    def self.sql(statement)
      return nil if Thread.current[:active_support_publishing_sql] == true
      Thread.current[:active_support_publishing_sql] = true
      begin
        instrument("sql.active_record", { sql: statement, name: "SQL" }) { 0 }
      ensure
        Thread.current[:active_support_publishing_sql] = false
      end
      nil
    end

    def self.subscribed(callback, name, monotonic: false)
      _ = monotonic
      if name == "sql.active_record"
        result = nil
        statements = Db.capture_sql { result = yield }
        i = 0
        while i < statements.length
          payload = { sql: statements[i], name: "SQL" }
          if callback.arity == 1
            now = Time.now
            callback.call(Event.new(name, now, now, nil, payload, 0.0))
          else
            callback.call(name, nil, nil, nil, payload)
          end
          i += 1
        end
        return result
      end
      subscriber = subscribe(name, callback)
      begin
        yield
      ensure
        unsubscribe(subscriber)
      end
    end

    class Event
      attr_reader :name, :time, :end, :transaction_id, :payload

      def initialize(name, start, ending, transaction_id, payload, span = nil)
        @name = name
        @time = start
        @end = ending
        @transaction_id = transaction_id
        @payload = payload
        # Seconds between start and finish, computed by `instrument`.
        @span = span.nil? ? 0.0 : span
      end

      # Milliseconds, Rails' `Event#duration`.
      def duration
        @span * 1000
      end
    end
  end
end
