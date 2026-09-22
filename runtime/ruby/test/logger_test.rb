# `runtime/ruby/logger.rb` against Ruby's own `Logger` and Rails'
# `TaggedLogging`.
#
# THE EXPECTED BYTES WERE MINTED BY RUBY, not by this port: each line
# below came out of `ruby -rlogger -e 'Logger::Formatter.new.call(…)'`
# with the pid substituted, and the tagged line out of campfire under
# Rails 8.2. So a match is interoperation — an emitted binary's stdout
# is the artifact a Rails process writes — rather than the port merely
# agreeing with itself.
#
# The pid is the one field a test cannot pin, so it is replaced in both
# the expected and the actual before comparing; everything else,
# including the two spaces `%5s` puts before INFO, is compared whole.
require "minitest/autorun"
require "stringio"
require_relative "test_helper"
require_relative "../logger"

class LoggerTest < Minitest::Test
  # 2026-09-22T08:41:37.086069 UTC — a fixed instant so the formatted
  # timestamp is a literal rather than a shape.
  TIME = Time.utc(2026, 9, 22, 8, 41, 37, 86_069)

  def depid(line)
    line.sub(/#\d+/, "#PID")
  end

  def test_formatter_renders_rubys_line_for_every_severity
    f = ::Logger::Formatter.new
    {
      "DEBUG" => "D, [2026-09-22T08:41:37.086069 #PID] DEBUG -- app: hello\n",
      "INFO"  => "I, [2026-09-22T08:41:37.086069 #PID]  INFO -- app: hello\n",
      "WARN"  => "W, [2026-09-22T08:41:37.086069 #PID]  WARN -- app: hello\n",
      "ERROR" => "E, [2026-09-22T08:41:37.086069 #PID] ERROR -- app: hello\n",
      "FATAL" => "F, [2026-09-22T08:41:37.086069 #PID] FATAL -- app: hello\n",
    }.each do |severity, expected|
      assert_equal expected, depid(f.call(severity, TIME, "app", "hello"))
    end
  end

  # Rails' tagged logger passes no progname, and Ruby renders that as an
  # empty field — `-- : m`, which is the shape a log reader keys on.
  def test_formatter_renders_an_absent_progname_as_an_empty_field
    f = ::Logger::Formatter.new
    assert_equal "I, [2026-09-22T08:41:37.086069 #PID]  INFO -- : m\n",
                 depid(f.call("INFO", TIME, nil, "m"))
  end

  def test_logger_writes_the_formatted_line_to_its_io
    io = StringIO.new
    ActiveSupport::Logger.new(io).info("hello")
    assert_match(/\AI, \[\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6} #\d+\]  INFO -- : hello\n\z/,
                 io.string)
  end

  # The slot is typed as the base class and holds a subclass, which is
  # the one shape in this file that a strict target has to get right —
  # campfire's `LogScrubbingFormatter` is dispatched through it and
  # reaches `Formatter#call` with `super`.
  class Scrubbing < ::Logger::Formatter
    def call(severity, time, progname, message)
      super.gsub("secret", "[FILTERED]")
    end
  end

  def test_an_app_subclass_in_the_formatter_slot_is_dispatched_through
    io = StringIO.new
    logger = ActiveSupport::Logger.new(io)
    logger.formatter = Scrubbing.new
    logger.info("a secret value")
    assert_includes io.string, "a [FILTERED] value"
    refute_includes io.string, "secret"
  end

  # The whole of campfire's production.rb wiring, and the line Rails
  # produced for it: the tags ride in the MESSAGE, so the progname
  # field stays empty.
  def test_tagged_logging_puts_its_tags_in_the_message
    io = StringIO.new
    logger = ActiveSupport::TaggedLogging.new(
      ActiveSupport::Logger.new(io).tap { |l| l.formatter = Scrubbing.new }
    )
    logger.tagged("req-123") { logger.info("a secret value") }
    assert_match(/ INFO -- : \[req-123\] a \[FILTERED\] value\n\z/, io.string)
  end

  def test_tags_nest_and_are_popped
    io = StringIO.new
    logger = ActiveSupport::TaggedLogging.new(ActiveSupport::Logger.new(io))
    logger.tagged("a") { logger.tagged("b") { logger.info("x") } }
    logger.info("y")
    lines = io.string.lines
    assert_includes lines[0], "[a] [b] x"
    assert_includes lines[1], ": y"
    # The tags are gone, not merely different: `[` alone would match
    # the timestamp field this format always carries.
    refute_includes lines[1], "[a]"
    refute_includes lines[1], "[b]"
  end

  # A raise inside the block still pops: one failed request must not
  # tag every line written after it.
  def test_a_raise_inside_tagged_still_pops
    io = StringIO.new
    logger = ActiveSupport::TaggedLogging.new(ActiveSupport::Logger.new(io))
    assert_raises(RuntimeError) { logger.tagged("req") { raise "boom" } }
    logger.info("after")
    refute_includes io.string, "[req]"
  end
end
