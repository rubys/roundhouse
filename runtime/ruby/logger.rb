# Ruby's `Logger` and the two ActiveSupport wrappers a Rails app puts
# in front of it — the logger stack `config.logger =` builds.
#
# WHY THIS IS HERE AT ALL. campfire's production.rb is
#
#   config.logger = ActiveSupport::Logger.new(STDOUT)
#     .tap  { |logger| logger.formatter = LogScrubbingFormatter.new }
#     .then { |logger| ActiveSupport::TaggedLogging.new(logger) }
#
# and `LogScrubbingFormatter < ::Logger::Formatter` exists because a bot
# request carries its KEY as a path segment (`/rooms/1/5-Ab3xK9mQz1Rt/
# messages`), which `config.filter_parameters` never sees — it redacts
# query and form parameters, not paths. So the formatter is the only
# thing between a bot key and the request log, and its `call` is
# `scrub(super)`: it needs the PARENT's rendering to scrub. With no
# `Logger::Formatter` in the tree that `super` is a NoMethodError, which
# is what campfire's own `log_scrubbing_formatter_test` reported on both
# lanes.
#
# `logger` IS A STDLIB THE STRICT TARGETS DO NOT HAVE. spinel says so
# out loud ("'logger' is not available in Spinel; the require is ignored
# and code using it will fail"), and the transpiled runtimes have no
# stdlib at all — so this is the ipaddr/zlib/resolv arrangement: one
# port, every target, ours. Unlike those three it is NOT swapped for
# Ruby's own on the CRuby/JRuby trees: an app subclasses
# `Logger::Formatter`, and two definitions of that constant is a
# superclass mismatch at load rather than a fallback.
#
# THE LINE FORMAT IS RUBY'S, MEASURED. `Logger::Formatter::Format` is
#
#   "%s, [%s #%d] %5s -- %s: %s\n"
#
# and against ruby 4.0 it renders
#
#   "I, [2026-09-22T08:41:37.086069 #69420]  INFO -- app: hello\n"
#
# (note the TWO spaces before INFO: `%5s` right-aligns in five). An
# emitted binary's stdout is then the same artifact a Rails process
# writes, which is what makes a log pipeline that parses one parse the
# other. `runtime/ruby/test/logger_test.rb` pins those bytes against
# values ruby minted.
#
# WHAT IS NOT MODELED, and why each is absent rather than stubbed:
#
# * LEVELS. `Logger#level` and the `debug?`/`info?` predicates gate
#   whether a line is written at all. Nothing in the corpus sets a
#   level, Rails' own default in production is `:info`, and a gate that
#   silently drops lines is the failure that looks like success — so
#   every level writes, and a corpus that sets one is what should add
#   the field.
# * THE BLOCK FORM (`logger.info { expensive }`). Its whole purpose is
#   to skip the expensive render below the level, and there is no level
#   to be below. An optional block is its own hazard on the strict
#   targets (docs/pipeline/runtime.md).
# * `Logger#add`, `<<`, `progname=`, the datetime format accessors, log
#   rotation. No caller.
require "stringio"

class Logger
  # Ruby's default formatter. An app subclasses THIS — that is the
  # whole reason it is a class with a body rather than a function.
  class Formatter
    # `severity[0..0]`, the datetime, the pid, `%5s` of the severity,
    # the progname and the message. Spelled with `+` rather than a
    # format string because `%` on a String is a shape the transpiled
    # targets do not all lower, and the widths here are two: one
    # right-align and one fixed-width timestamp.
    #
    # `progname` reads as "" when the caller passed none, which is what
    # Rails' tagged logger does — it puts the tags in the MESSAGE and
    # leaves the progname empty, so the line says `-- : [tag] msg`.
    def call(severity, time, progname, message)
      severity.to_s[0, 1] + ", [" + Logger::Formatter.format_datetime(time) +
        " #" + Process.pid.to_s + "] " + severity.to_s.rjust(5) +
        " -- " + progname.to_s + ": " + message.to_s + "\n"
    end

    # `%Y-%m-%dT%H:%M:%S.%6N` — Ruby's `Formatter::DatetimeFormat`
    # with its microsecond field. Fixed width in a single zone, so a
    # reader that sorts these sorts chronologically.
    def self.format_datetime(time)
      time.strftime("%Y-%m-%dT%H:%M:%S.%6N")
    end
  end
end

module ActiveSupport
  # `ActiveSupport::Logger.new(io)`. Rails' own subclasses Ruby's
  # `Logger` and adds the broadcast machinery; what an app reaches for
  # is the constructor, the formatter slot and the five level methods.
  #
  # THE FORMATTER SLOT IS TYPED AS THE BASE CLASS and holds a subclass —
  # the one place this file's shape is load-bearing on a strict target.
  # An app's `LogScrubbingFormatter` is dispatched through it, and its
  # `super` reaches `Formatter#call` above.
  class Logger
    def initialize(io)
      @io = io
      @formatter = ::Logger::Formatter.new
      @progname = ""
    end

    def formatter
      @formatter
    end

    def formatter=(value)
      @formatter = value
      value
    end

    # The one write path. `Time.now` per line, as Ruby's does.
    def write_entry(severity, message)
      @io.write(@formatter.call(severity, Time.now, @progname, message))
      nil
    end

    def debug(message)
      write_entry("DEBUG", message)
    end

    def info(message)
      write_entry("INFO", message)
    end

    def warn(message)
      write_entry("WARN", message)
    end

    def error(message)
      write_entry("ERROR", message)
    end

    def fatal(message)
      write_entry("FATAL", message)
    end
  end

  # `ActiveSupport::TaggedLogging.new(logger)` — the wrapper that puts
  # `[req-123] ` in front of every line written inside `tagged`.
  #
  # A WRAPPER, NOT A MODULE EXTENDED ONTO THE LOGGER. Rails does
  # `logger.extend(TaggedLogging)`, which is a per-object method table
  # no compiled target has (`lower::object_extend`); `new` returning a
  # wrapper is the same surface reached by a shape every target
  # compiles, and it is what `TaggedLogging.new` already reads as at
  # the call site.
  #
  # THE TAGS GO IN THE MESSAGE, not the progname — measured against
  # Rails, whose line reads `-- : [req-123] msg`.
  class TaggedLogging
    def initialize(logger)
      @logger = logger
      @tags = []
    end

    # `logger.tagged("req-123") { … }`. Rails nests, so the tag is
    # pushed and popped rather than assigned; an exception inside the
    # block still pops, which is what keeps one failed request from
    # tagging every line after it.
    def tagged(tag)
      @tags.push(tag.to_s)
      begin
        yield
      ensure
        @tags.pop
      end
    end

    def formatter
      @logger.formatter
    end

    def formatter=(value)
      @logger.formatter = value
      value
    end

    def tagged_message(message)
      return message.to_s if @tags.empty?
      prefix = +""
      i = 0
      while i < @tags.length
        prefix = prefix + "[" + @tags[i] + "] "
        i = i + 1
      end
      prefix + message.to_s
    end

    def debug(message)
      @logger.debug(tagged_message(message))
    end

    def info(message)
      @logger.info(tagged_message(message))
    end

    def warn(message)
      @logger.warn(tagged_message(message))
    end

    def error(message)
      @logger.error(tagged_message(message))
    end

    def fatal(message)
      @logger.fatal(tagged_message(message))
    end
  end
end
