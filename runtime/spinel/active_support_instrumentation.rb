# frozen_string_literal: true
# Not in runtime/ruby/active_support_notifications.rb: these keep per-thread state the strict targets' typing gate cannot follow, and only the ruby family instruments.
module ActiveSupport
  module Notifications
    # A statement the shim prepares and steps: the event opened here publishes on `sql_close`, and one still open when the next opens is closed first.
    def self.sql_open(statement)
      return nil unless @sql_listening
      return nil if Thread.current[:active_support_publishing_sql] == true
      sql_close
      now = monotonic_now
      Thread.current[:active_support_sql_open] = statement
      Thread.current[:active_support_sql_since] = now
      Thread.current[:active_support_sql_spent] = 0.0
      nil
    end

    def self.sql_prepared
      return nil unless @sql_listening
      since = Thread.current[:active_support_sql_since]
      return nil if since.nil?
      Thread.current[:active_support_sql_spent] = Thread.current[:active_support_sql_spent] + (monotonic_now - since)
      nil
    end

    def self.sql_step_begin
      return 0.0 unless @sql_listening
      return 0.0 if Thread.current[:active_support_sql_open].nil?
      monotonic_now
    end

    def self.sql_step_end(started, done)
      return nil if started == 0.0 || Thread.current[:active_support_sql_open].nil?
      Thread.current[:active_support_sql_spent] = Thread.current[:active_support_sql_spent] + (monotonic_now - started)
      sql_close if done
      nil
    end

    def self.sql_close
      return nil unless @sql_listening
      statement = Thread.current[:active_support_sql_open]
      return nil if statement.nil?
      spent = Thread.current[:active_support_sql_spent]
      Thread.current[:active_support_sql_open] = nil
      Thread.current[:active_support_sql_since] = nil
      sql_publish(statement, spent, false)
    end

    # A query-cache hit: Rails publishes it too, `cached: true`, for the lookup's time.
    def self.sql_cached(statement)
      return nil unless @sql_listening
      return nil if Thread.current[:active_support_publishing_sql] == true
      sql_close
      sql_publish(statement, 0.0, true)
    end

    def self.sql_publish(statement, spent, cached)
      Thread.current[:active_support_publishing_sql] = true
      begin
        finish = monotonic_now
        # Not `cached: false` on a statement that ran: Rails' payload has the key only for a query-cache hit.
        payload = cached ? { sql: statement, name: "SQL", cached: true } : { sql: statement, name: "SQL" }
        publish("sql.active_record", finish - spent, finish, payload)
      ensure
        Thread.current[:active_support_publishing_sql] = false
      end
      nil
    end

    # 0.0 when nothing listens, so `render_finish` skips the publish too.
    def self.render_start
      return 0.0 unless @render_listening
      depth = Thread.current[:active_support_render_depth]
      Thread.current[:active_support_render_depth] = (depth.nil? ? 0 : depth) + 1
      monotonic_now
    end

    def self.render_finish(name, identifier, started)
      return nil if started == 0.0
      depth = Thread.current[:active_support_render_depth]
      depth = depth.nil? ? 1 : depth
      Thread.current[:active_support_render_depth] = depth - 1
      items = Thread.current[:active_support_render_collection_depths]
      # Not published per item: Rails' collection renderer reports the collection once, as render_collection.
      if !items.nil? && !items.empty? && items[items.length - 1] == depth
        counts = Thread.current[:active_support_render_collection_counts]
        counts[counts.length - 1] = counts[counts.length - 1] + 1
        return nil
      end
      publish(name, started, monotonic_now, { identifier: identifier })
    end

    def self.collection_start
      return 0.0 unless @render_listening
      depth = Thread.current[:active_support_render_depth]
      items = Thread.current[:active_support_render_collection_depths]
      counts = Thread.current[:active_support_render_collection_counts]
      if items.nil?
        items = [0]
        items.pop
        counts = [0]
        counts.pop
        Thread.current[:active_support_render_collection_depths] = items
        Thread.current[:active_support_render_collection_counts] = counts
      end
      items << (depth.nil? ? 0 : depth) + 1
      counts << 0
      monotonic_now
    end

    def self.collection_finish(identifier, started)
      return nil if started == 0.0
      items = Thread.current[:active_support_render_collection_depths]
      counts = Thread.current[:active_support_render_collection_counts]
      items.pop
      count = counts.pop
      # Rails' collection renderer returns before instrumenting an empty collection.
      return nil if count == 0
      publish("render_collection.action_view", started, monotonic_now, { identifier: identifier, count: count })
    end

    def self.start_processing(controller, action)
      return 0.0 unless @process_listening
      now = monotonic_now
      publish("start_processing.action_controller", now, now, { controller: controller, action: action })
      now
    end

    def self.process_action(controller, action, started)
      return nil if started == 0.0
      publish("process_action.action_controller", started, monotonic_now, { controller: controller, action: action })
    end
  end
end
