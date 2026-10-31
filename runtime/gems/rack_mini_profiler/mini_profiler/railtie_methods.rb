# frozen_string_literal: true
module Rack
  class MiniProfiler
    # ProfilingMethods#record_sql and the Railtie's subscribers (Rack::MiniProfilerRails.initialize! / railtie_methods.rb).
    module RailtieMethods
      def self.current
        ctx = Thread.current[:mini_profiler_private]
        return nil if ctx.nil? || ctx.mode != Context::PROFILE
        ctx
      end

      def self.record_sql(query, elapsed_ms, cached)
        ctx = current
        return nil if ctx.nil?
        page = ctx.page
        timer = ctx.current_timer
        return nil if page.nil? || timer.nil?
        page.add_sql(timer, query, elapsed_ms, cached)
      end

      def self.start_processing(controller, action)
        ctx = current
        return nil if ctx.nil?
        page = ctx.page
        timer = ctx.current_timer
        return nil if page.nil? || timer.nil?
        name = controller.end_with?("Controller") ? controller[0, controller.length - "Controller".length] : controller
        ctx.processing_parent = timer
        ctx.current_timer = page.add_child(timer, "Executing: " + name.downcase + "#" + action)
        nil
      end

      def self.process_action
        ctx = current
        return nil if ctx.nil?
        page = ctx.page
        timer = ctx.current_timer
        parent = ctx.processing_parent
        return nil if page.nil? || timer.nil? || parent.nil?
        ctx.processing_parent = nil
        timer.record_time((Process.clock_gettime(Process::CLOCK_MONOTONIC) - timer.start) * 1000, page.children_ms(timer))
        ctx.current_timer = parent
        nil
      end

      def self.render_notification(identifier, finish, start)
        ctx = current
        return nil if ctx.nil?
        page = ctx.page
        parent = ctx.current_timer
        return nil if page.nil? || parent.nil?
        parts = identifier.split("/")
        name = parts.length > 1 ? parts[parts.length - 2] + "/" + parts[parts.length - 1] : identifier
        node = page.add_child(parent, "Rendering: " + name)
        duration = finish - start
        duration_ms = duration * 1000
        node.start -= duration
        node.start_milliseconds -= duration_ms
        node.start_float = true
        node.record_time(duration_ms, 0.0)
        children_duration = 0.0
        moved = page.children_of(parent).select { |child| !child.equal?(node) && RailtieMethods.should_move?(child.start_milliseconds, child.duration_milliseconds, node) }
        moved.each { |child| children_duration += child.duration_milliseconds }
        node.duration_without_children_milliseconds = duration_ms - children_duration
        moved.each { |child| page.move_child(child, node) }
        page.sqls_of(parent).each do |sql|
          page.move_sql(sql, node) if RailtieMethods.should_move?(sql.start_milliseconds, sql.duration_milliseconds, node)
        end
        nil
      end

      def self.should_move?(start, duration, node)
        start >= node.start_milliseconds &&
          start + duration <= node.start_milliseconds + node.duration_milliseconds
      end

      def self.subscribe
        ActiveSupport::Notifications.monotonic_subscribe("start_processing.action_controller") do |_name, _start, _finish, _id, payload|
          RailtieMethods.start_processing(payload[:controller].to_s, payload[:action].to_s)
        end
        ActiveSupport::Notifications.monotonic_subscribe("process_action.action_controller") do |_name, _start, _finish, _id, _payload|
          RailtieMethods.process_action
        end
        ActiveSupport::Notifications.monotonic_subscribe("render_partial.action_view") do |_name, start, finish, _id, payload|
          RailtieMethods.render_notification(payload[:identifier].to_s, finish, start)
        end
        ActiveSupport::Notifications.monotonic_subscribe("render_template.action_view") do |_name, start, finish, _id, payload|
          RailtieMethods.render_notification(payload[:identifier].to_s, finish, start)
        end
        # `subscribe_sql_active_record`: the gem patches no driver here (no Rails patches asked for), so it hears the event.
        ActiveSupport::Notifications.monotonic_subscribe("sql.active_record") do |_name, start, finish, _id, payload|
          RailtieMethods.record_sql(payload[:sql].to_s, (finish - start) * 1000, payload[:cached] == true)
        end
        nil
      end
    end
  end
end
