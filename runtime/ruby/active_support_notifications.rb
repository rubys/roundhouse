# frozen_string_literal: true
# `ActiveSupport::Notifications`. Subscribe for the block, unsubscribe
# after. A one-argument callback receives an Event; any other arity
# receives the five arguments Rails passes, `Time`s, or monotonic
# `Float` seconds for a `monotonic_subscribe`r.
#
# `Db.record_query` calls `sql` (or, on the Spinel SQLite shim, the timed
# `sql_open` of runtime/spinel/active_support_instrumentation.rb), so a
# `subscribe("sql.active_record")` hears each statement. `subscribed` for
# that name still drains `Db.capture_sql` after the block. The payload
# carries `:sql` and `:name` (`"SQL"`), and `cached: true` for a query-cache hit.
module ActiveSupport
  module Notifications
    SUBSCRIBERS = []
    # Not a lookup per event: a pattern may be a Regexp, and the instrumented paths run per statement, per step and per view.
    @sql_listening = false
    @render_listening = false
    @process_listening = false

    class Subscriber
      attr_reader :pattern, :listener, :monotonic

      def initialize(pattern, listener, monotonic)
        @pattern = pattern
        @listener = listener
        @monotonic = monotonic
      end

      def matches?(name)
        @pattern === name
      end

      def notify(name, start, finish, wall, payload)
        if @monotonic
          @listener.call(name, start, finish, nil, payload)
        elsif @listener.arity == 1
          @listener.call(Event.new(name, Time.at(wall + start), Time.at(wall + finish), nil, payload, finish - start))
        else
          @listener.call(name, Time.at(wall + start), Time.at(wall + finish), nil, payload)
        end
        nil
      end
    end

    def self.subscribe(pattern, callback = nil, monotonic: false, &block)
      listener = callback.nil? ? block : callback
      subscriber = Subscriber.new(pattern, listener, monotonic)
      SUBSCRIBERS << subscriber
      refresh_listening
      subscriber
    end

    def self.monotonic_subscribe(pattern, callback = nil, &block)
      subscribe(pattern, callback, monotonic: true, &block)
    end

    def self.unsubscribe(subscriber)
      SUBSCRIBERS.delete(subscriber)
      refresh_listening
      nil
    end

    def self.refresh_listening
      @sql_listening = listening?("sql.active_record")
      @render_listening = listening?("render_template.action_view") || listening?("render_partial.action_view") ||
        listening?("render_layout.action_view") || listening?("render_collection.action_view")
      @process_listening = listening?("start_processing.action_controller") ||
        listening?("process_action.action_controller")
      nil
    end

    def self.sql_listening?
      @sql_listening
    end

    def self.render_listening?
      @render_listening
    end

    def self.process_listening?
      @process_listening
    end

    def self.subscribers
      SUBSCRIBERS
    end

    def self.listening?(name)
      list = subscribers
      i = 0
      while i < list.length
        return true if list[i].matches?(name)
        i += 1
      end
      false
    end

    def self.monotonic_now
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end

    def self.instrument(name, payload = nil, &block)
      payload = {} if payload.nil?
      start = monotonic_now
      value = nil
      begin
        value = block.nil? ? nil : yield
      ensure
        publish(name, start, monotonic_now, payload)
      end
      value
    end

    def self.publish(name, start, finish, payload)
      wall = Time.now.to_f - monotonic_now
      list = subscribers
      i = 0
      while i < list.length
        entry = list[i]
        entry.notify(name, start, finish, wall, payload) if entry.matches?(name)
        i += 1
      end
      nil
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
