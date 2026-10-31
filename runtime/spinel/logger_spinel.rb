# frozen_string_literal: true
# Ruby's `Logger.new(io)` and its level methods on spinel, where
# runtime/logger.rb's `::Logger` is only the Formatter an app subclasses.
# An initializer's own `Logger.new($stdout).warn "…"` runs at boot here
# (ingest::boot_program). The ruby family loads Ruby's Logger, which this
# file must not reopen: only spinel's boot.rb requires it.
class Logger
  def initialize(io)
    @io = io
    @formatter = Logger::Formatter.new
    @progname = ""
  end

  def formatter
    @formatter
  end

  def formatter=(value)
    @formatter = value
    value
  end

  def write_entry(severity, message)
    @io << @formatter.call(severity, Time.now, @progname, message)
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
