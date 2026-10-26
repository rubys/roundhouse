# frozen_string_literal: true

# ActiveSupport's two process-wide reporters, ported from activesupport:
# `Rails.event` (EventReporter, active_support/event_reporter.rb) and
# `Rails.error` (ErrorReporter, active_support/error_reporter.rb), with the
# ParameterFilter that redacts event payloads
# (active_support/parameter_filter.rb) and the ExecutionContext an error
# report reads its context from (active_support/execution_context.rb).
# `Rails.error` / `Rails.event` answer the reporters below as railties'
# lib/rails.rb does (`ActiveSupport.error_reporter` / `.event_reporter`).
#
# Metaprogramming-free like rails.rb, with these deliberate differences:
#
# - Rails keeps event tags, event context and event debug mode in Fiber
#   storage (inherited by fibers and threads a fiber starts) and the
#   execution context in IsolatedExecutionState. Here all of it lives in
#   `Thread.current[]`, which is fiber-local and not inherited: a block
#   run in a new thread or fiber starts with empty tags and context.
# - `rescue *error_classes => e` is written as `rescue Exception => e`
#   plus a class test that re-raises anything not listed; the set of
#   errors handled is the same.
# - The reported-error flag (`@__rails_error_reported`) is kept beside the
#   error, in REPORTED, by object_id.
# - ExecutionContext has no `current_attributes_instances`: nothing here
#   defines CurrentAttributes, which is what registers them.
# - A report to an ErrorReporter with no subscribers is logged through
#   Rails.logger, where Rails drops it (see ErrorReporter#report).
# - ensure_backtrace trims this file's frames from `backtrace` by their
#   "path:" prefix and leaves `backtrace_locations` whole; Rails reads the
#   locations' paths and shifts both. Spinel has no backtrace_locations.
#
# Released activesupport (8.1) and the main-branch reporters differ
# in the places listed here, so a reader can tell what each file is
# following. The 8.2-only parts are additive:
#
# - EventReporter#notify / #debug take `filter_payload:`, and
#   `EventReporter.filter_parameters` overrides ActiveSupport.filter_parameters
#   for one reporter (8.1 filters every Hash payload with the global list).
# - ErrorReporter hands each subscriber a deep_dup of the context (8.1
#   passes the one Hash to all of them).
# - Event debug mode starts on (8.1: off until the railtie sets it from the
#   log level); an emitted program has no railtie to set it.
#
# `ActiveSupport.filter_parameters` starts empty, as in Rails before the
# railtie copies `config.filter_parameters` into it.
module ActiveSupport
  class << self
    attr_writer :error_reporter, :event_reporter, :filter_parameters
  end

  def self.error_reporter
    @error_reporter ||= ErrorReporter.new
  end

  def self.event_reporter
    @event_reporter ||= EventReporter.new
  end

  def self.filter_parameters
    @filter_parameters ||= []
  end

  # active_support/parameter_filter.rb. A String or Symbol filter matches
  # any key containing it, case-insensitively; a Regexp matches as
  # written; a filter with a `.` matches the dotted path from the root
  # (`"card.code"`); a Proc is called with the key and value (and the
  # original params when it takes three) to rewrite the value in place.
  class ParameterFilter
    FILTERED = "[FILTERED]"

    def initialize(filters = [], mask: FILTERED)
      @mask = mask
      @no_filters = filters.empty?
      @regexps = []
      @deep_regexps = []
      @blocks = filters.select { |item| item.is_a?(Proc) }
      strings = []
      deep_strings = []
      filters.each do |item|
        next if item.is_a?(Proc)
        if item.is_a?(Regexp)
          if item.to_s.include?("\\.")
            @deep_regexps << item
          else
            @regexps << item
          end
        else
          s = Regexp.escape(item.to_s)
          if s.include?("\\.")
            deep_strings << s
          else
            strings << s
          end
        end
      end
      @regexps << Regexp.new(strings.join("|"), Regexp::IGNORECASE) unless strings.empty?
      @deep_regexps << Regexp.new(deep_strings.join("|"), Regexp::IGNORECASE) unless deep_strings.empty?
    end

    def filter(params)
      @no_filters ? params.dup : filter_hash(params, nil, params)
    end

    def filter_param(key, value)
      @no_filters ? value : value_for_key(key, value, nil, nil)
    end

    private

    def filter_hash(params, full_parent_key, original_params)
      filtered = {}
      params.each do |key, value|
        filtered[key] = value_for_key(key, value, full_parent_key, original_params)
      end
      filtered
    end

    def value_for_key(key, value, full_parent_key, original_params)
      # One `to_s` of the untyped key. The path and the regexp both use it.
      key_text = key.to_s
      full_key = full_parent_key.nil? ? key_text : "#{full_parent_key}.#{key_text}"
      # `=~`, not `match?`: Spinel types `r` as poly (the filters arrive
      # untyped), and a poly `match?` dispatch reaches PlatformAgent's
      # private `match?(pattern)` with a String, after which Spinel
      # compiles that method's own `String#match?` as an unconditional
      # NoMethodError (campfire's /rooms/1 under a Chrome user agent;
      # matz/spinel#8633).
      if @regexps.any? { |r| !(r =~ key_text).nil? }
        @mask
      elsif @deep_regexps.any? { |r| !(r =~ full_key).nil? }
        @mask
      elsif value.is_a?(Hash)
        filter_hash(value, full_key, original_params)
      elsif value.is_a?(Array)
        value.map { |v| value_for_key(key, v, full_parent_key, original_params) }
      elsif @blocks.empty?
        value
      else
        k = key.is_a?(String) ? key.dup : key
        v = value.is_a?(String) ? value.dup : value
        @blocks.each { |b| b.arity == 2 ? b.call(k, v) : b.call(k, v, original_params) }
        v
      end
    end
  end

  # active_support/execution_context.rb: the per-thread context hash an
  # error report merges under its own `context:`.
  module ExecutionContext
    class Record
      attr_reader :store

      def initialize
        @store = {}
        @stack = []
      end

      def push
        @stack << @store
        @store = {}
        self
      end

      def pop
        @store = @stack.pop || {}
        self
      end

      def flush
        @stack = @stack.map { |_| {} }
        @store = {}
        self
      end
    end

    class << self
      attr_writer :nestable
    end

    def self.nestable
      @nestable || false
    end

    def self.after_change(&block)
      after_change_callbacks << block
    end

    def self.after_change_callbacks
      @after_change_callbacks ||= []
    end

    def self.changed
      after_change_callbacks.each(&:call)
    end

    def self.set(**options)
      store = record.store
      previous = {}
      options.each do |key, _|
        previous[key.to_sym] = store[key.to_sym]
      end
      options.each { |key, value| store[key.to_sym] = value }
      changed
      return nil unless block_given?
      begin
        yield
      ensure
        store.merge!(previous)
        changed
      end
    end

    def self.[]=(key, value)
      record.store[key.to_sym] = value
      changed
    end

    def self.to_h
      record.store.dup
    end

    def self.push
      nestable ? record.push : clear
      changed
      self
    end

    def self.pop
      nestable ? record.pop : clear
      changed
      self
    end

    def self.clear
      Thread.current[:active_support_execution_context] = nil
      changed
      nil
    end

    def self.flush
      result = record.flush
      changed
      result
    end

    def self.record
      current = Thread.current[:active_support_execution_context]
      return current unless current.nil?
      fresh = Record.new
      Thread.current[:active_support_execution_context] = fresh
      fresh
    end
  end

  # The event tags `Rails.event.tagged` pushes for the length of a block.
  class TagStack
    EMPTY_TAGS = {}.freeze

    def self.tags
      Thread.current[:event_reporter_tags] || EMPTY_TAGS
    end

    def self.with_tags(args, kwargs)
      existing = tags
      merged = existing.dup
      resolve_tags(args, kwargs).each { |key, value| merged[key] = value }
      begin
        Thread.current[:event_reporter_tags] = merged.freeze
        yield
      ensure
        Thread.current[:event_reporter_tags] = existing
      end
    end

    def self.resolve_tags(args, kwargs)
      tags = {}
      args.each do |arg|
        if arg.is_a?(String)
          tags[arg.to_sym] = true
        elsif arg.is_a?(Symbol)
          tags[arg] = true
        elsif arg.is_a?(Hash)
          arg.each { |key, value| tags[key.to_sym] = value }
        else
          tags[arg.class.name.to_sym] = arg
        end
      end
      kwargs.each { |key, value| tags[key.to_sym] = value }
      tags
    end
  end

  # The event context `Rails.event.set_context` merges into.
  class EventContext
    EMPTY_CONTEXT = {}.freeze

    def self.context
      Thread.current[:event_reporter_context] || EMPTY_CONTEXT
    end

    def self.set_context(context_hash)
      updated = context.dup
      context_hash.each { |key, value| updated[key.to_sym] = value }
      Thread.current[:event_reporter_context] = updated.freeze
    end

    def self.clear
      Thread.current[:event_reporter_context] = EMPTY_CONTEXT
    end
  end

  class EventReporter
    attr_writer :raise_on_error, :debug_mode
    attr_reader :subscribers

    def self.context_store
      @context_store ||= EventContext
    end

    def self.context_store=(store)
      @context_store = store
    end

    # The filters this reporter's payloads are redacted with, in place of
    # ActiveSupport.filter_parameters (`reload_payload_filter` to apply).
    def self.filter_parameters
      @filter_parameters
    end

    def self.filter_parameters=(filters)
      @filter_parameters = filters
    end

    # Debug events are on until the app's boot sets `debug_mode` (Rails'
    # `initialize_event_reporter`: on at log level :debug).
    def initialize(*subscribers, raise_on_error: false)
      @subscribers = []
      subscribers.each { |subscriber| subscribe(subscriber) }
      @debug_mode = true
      @raise_on_error = raise_on_error
      @payload_filter = nil
    end

    def subscribe(subscriber, &block)
      unless subscriber.respond_to?(:emit)
        raise ArgumentError, "Event subscriber #{subscriber.class.name} must respond to #emit"
      end
      @subscribers << { subscriber: subscriber, filter: block }
    end

    def unsubscribe(subscriber)
      @subscribers.delete_if { |s| subscriber === s[:subscriber] }
    end

    def notify(name_or_object, payload = nil, caller_depth: 1, filter_payload: true, **kwargs)
      deliver(name_or_object, payload, kwargs, filter_payload, caller_depth + 1)
    end

    def with_debug
      prior = Thread.current[:event_reporter_debug_mode]
      Thread.current[:event_reporter_debug_mode] = true
      begin
        yield
      ensure
        Thread.current[:event_reporter_debug_mode] = prior
      end
    end

    def debug_mode?
      @debug_mode || Thread.current[:event_reporter_debug_mode]
    end

    def debug(name_or_object, payload = nil, caller_depth: 1, filter_payload: true, **kwargs)
      return nil unless debug_mode?
      if block_given?
        merged = kwargs.dup
        yield.each { |key, value| merged[key] = value }
        deliver(name_or_object, payload, merged, filter_payload, caller_depth + 1)
      else
        deliver(name_or_object, payload, kwargs, filter_payload, caller_depth + 1)
      end
    end

    def tagged(*args, **kwargs)
      TagStack.with_tags(args, kwargs) { yield }
    end

    def set_context(context)
      self.class.context_store.set_context(context)
    end

    def clear_context
      self.class.context_store.clear
    end

    def context
      self.class.context_store.context
    end

    def reload_payload_filter
      @payload_filter = nil
      payload_filter
    end

    private

    # `notify` and `debug` both land here, `depth` frames above the
    # caller whose location the event records. With no subscriber,
    # nothing is built; a subscriber's filter sees the whole event, as
    # event_reporter.rb passes it.
    def deliver(name_or_object, payload, kwargs, filter_payload, depth)
      return nil if @subscribers.empty?
      # One pair of `is_a?` reads. `resolve_name` and `resolve_payload`
      # both asked the same question of the untyped name.
      named = name_or_object.is_a?(String) || name_or_object.is_a?(Symbol)
      name = named ? name_or_object.to_s : name_or_object.class.name

      event = {
        name: name,
        payload: resolve_payload(named, name_or_object, payload, kwargs, filter_payload),
        tags: TagStack.tags,
        context: self.class.context_store.context,
        timestamp: Process.clock_gettime(Process::CLOCK_REALTIME, :nanosecond),
      }
      locations = caller_locations(depth, 1)
      location = locations.nil? ? nil : locations.first
      unless location.nil?
        event[:source_location] = { filepath: location.path, lineno: location.lineno, label: location.label }
      end
      @subscribers.each do |entry|
        filter = entry[:filter]
        subscriber = entry[:subscriber]
        begin
          next if !filter.nil? && !filter.call(event)
          subscriber.emit(event)
        rescue StandardError => subscriber_error
          raise if @raise_on_error
          ActiveSupport.error_reporter.report(subscriber_error, handled: true)
        end
      end
      nil
    end

    def payload_filter
      @payload_filter ||= ParameterFilter.new(effective_filter_parameters, mask: ParameterFilter::FILTERED)
    end

    # Rails' private `filter_parameters`. Renamed here because the runtime
    # signature check keys a class's methods by name alone, and the class
    # has a public `filter_parameters` accessor of its own.
    def effective_filter_parameters
      self.class.filter_parameters || ActiveSupport.filter_parameters
    end

    def resolve_payload(named, name_or_object, payload, kwargs, filter)
      if named
        handle_unexpected_args(name_or_object, payload, kwargs) if !payload.nil? && kwargs.any?
        if kwargs.any?
          resolved = symbolize(kwargs)
          filter ? payload_filter.filter(resolved) : resolved
        elsif !payload.nil?
          resolved = symbolize(payload)
          filter ? payload_filter.filter(resolved) : resolved
        end
      else
        handle_unexpected_args(name_or_object, payload, kwargs) if !payload.nil? || kwargs.any?
        name_or_object
      end
    end

    def symbolize(hash)
      out = {}
      hash.each { |key, value| out[key.to_sym] = value }
      out
    end

    def handle_unexpected_args(name_or_object, payload, kwargs)
      message = "Rails.event.notify accepts either an event object, a payload hash, or keyword arguments.\n" \
        "Received: #{name_or_object.inspect}, #{payload.inspect}, #{kwargs.inspect}\n"
      raise ArgumentError, message if @raise_on_error
      ActiveSupport.error_reporter.report(ArgumentError.new(message), handled: true)
    end
  end

  class ErrorReporter
    SEVERITIES = [:error, :warning, :info].freeze
    DEFAULT_SOURCE = "application"
    DEFAULT_RESCUE = [StandardError].freeze
    # The errors already reported, by object_id: Rails flags each one
    # (and its causes) with `@__rails_error_reported` so a second report
    # anywhere is skipped. object_ids are never reused, so this keeps
    # the same answer without holding the errors. A frozen error cannot
    # be flagged in Rails and is not recorded here either.
    REPORTED = {}

    class UnexpectedError < Exception
    end

    # The context middlewares `add_middleware` stacks; each one's result
    # is the next one's `context:`.
    class ErrorContextMiddlewareStack
      def initialize(*middlewares)
        @stack = middlewares
      end

      def use(middleware)
        unless middleware.respond_to?(:call)
          raise ArgumentError, "Error context middleware must respond to #call"
        end
        @stack << middleware
      end

      def execute(error, handled:, severity:, context:, source:)
        result = context
        @stack.each do |middleware|
          result = middleware.call(error, context: result, handled: handled, severity: severity, source: source)
        end
        result
      end
    end

    attr_accessor :logger, :debug_mode

    def initialize(*subscribers, logger: nil)
      @subscribers = subscribers.flatten
      @logger = logger
      @debug_mode = false
      @context_middlewares = ErrorContextMiddlewareStack.new
    end

    def handle(*error_classes, severity: :warning, context: {}, fallback: nil, source: DEFAULT_SOURCE)
      handled = error_classes.empty? ? DEFAULT_RESCUE : error_classes
      begin
        yield
      rescue Exception => error
        raise unless handled.any? { |klass| error.is_a?(klass) }
        report(error, handled: true, severity: severity, context: context, source: source)
        fallback.nil? ? nil : fallback.call
      end
    end

    def record(*error_classes, severity: :error, context: {}, source: DEFAULT_SOURCE)
      handled = error_classes.empty? ? DEFAULT_RESCUE : error_classes
      begin
        yield
      rescue Exception => error
        raise unless handled.any? { |klass| error.is_a?(klass) }
        report(error, handled: false, severity: severity, context: context, source: source)
        raise
      end
    end

    def unexpected(error, severity: :warning, context: {}, source: DEFAULT_SOURCE)
      error = RuntimeError.new(error) if error.is_a?(String)
      if @debug_mode
        ensure_backtrace(error)
        raise UnexpectedError, "#{error.class.name}: #{error.message}", error.backtrace, cause: error
      else
        report(error, handled: true, severity: severity, context: context, source: source)
      end
    end

    def subscribe(subscriber)
      unless subscriber.respond_to?(:report)
        raise ArgumentError, "Error subscribers must respond to #report"
      end
      @subscribers << subscriber
    end

    def unsubscribe(subscriber)
      @subscribers.delete_if { |s| subscriber === s }
    end

    # The subscribers turned off for this thread, keyed by reporter as
    # Rails' IsolatedExecutionState[self] is.
    def disable(subscriber)
      disabled = disabled_subscribers
      disabled << subscriber
      begin
        yield
      ensure
        disabled.delete(subscriber)
      end
    end

    def set_context(**context)
      if block_given?
        ActiveSupport::ExecutionContext.set(**context) { yield }
      else
        ActiveSupport::ExecutionContext.set(**context)
      end
    end

    def add_middleware(middleware)
      @context_middlewares.use(middleware)
    end

    def report(error, handled: true, severity: handled ? :warning : :error, context: {}, source: DEFAULT_SOURCE)
      return nil if REPORTED[error.object_id]
      raise ArgumentError, "Reported error must be an Exception, got: #{error.inspect}" unless error.is_a?(Exception)
      ensure_backtrace(error)
      unless SEVERITIES.include?(severity)
        raise ArgumentError, "severity must be one of #{SEVERITIES.map(&:inspect).join(", ")}, got: #{severity.inspect}"
      end
      return log_unsubscribed(error, severity) if @subscribers.empty?
      base = ActiveSupport::ExecutionContext.to_h
      (context || {}).each { |key, value| base[key] = value }
      full_context = @context_middlewares.execute(error, context: base, handled: handled, severity: severity, source: source)
      disabled = disabled_for_thread
      @subscribers.each do |subscriber|
        begin
          unless !disabled.nil? && disabled.any? { |s| s === subscriber }
            subscriber.report(error, handled: handled, severity: severity, context: ActiveSupport.deep_dup(full_context), source: source)
          end
        rescue StandardError => subscriber_error
          raise if @logger.nil?
          @logger.fatal(
            "Error subscriber raised an error: #{subscriber_error.message} (#{subscriber_error.class})\n" +
            subscriber_error.backtrace.join("\n")
          )
        end
      end
      current = error
      until current.nil?
        REPORTED[current.object_id] = true unless current.frozen?
        current = current.cause
      end
      nil
    end

    private

    # Rails drops a report nothing subscribes to. The emitted program has
    # no error subscribers unless it adds one, so such a report is logged
    # through Rails.logger at its severity instead, as the one place the
    # error is not lost. Once anything subscribes, delivery is Rails' and
    # nothing is logged here. The error is not flagged as reported, so a
    # later subscriber still sees a repeat of it.
    def log_unsubscribed(error, severity)
      message = "#{error.class}: #{error.message}"
      if severity == :info
        Rails.logger.info(message)
      elsif severity == :warning
        Rails.logger.warn(message)
      else
        Rails.logger.error(message)
      end
      nil
    end

    def disabled_for_thread
      table = Thread.current[:active_support_error_reporter_disabled]
      table.nil? ? nil : table[object_id]
    end

    def disabled_subscribers
      table = Thread.current[:active_support_error_reporter_disabled]
      if table.nil?
        table = {}
        Thread.current[:active_support_error_reporter_disabled] = table
      end
      table[object_id] ||= []
    end

    # A never-raised error has no backtrace; raising it here gives it one
    # (and its `cause`), minus the frames of this file.
    def ensure_backtrace(error)
      return nil if error.frozen?
      return nil unless error.backtrace.nil?
      begin
        raise error
      rescue Exception
        nil
      end
      lines = error.backtrace
      prefix = "#{__FILE__}:"
      count = 0
      count += 1 while count < lines.length && lines[count].start_with?(prefix)
      lines.shift(count)
      nil
    end
  end
end
