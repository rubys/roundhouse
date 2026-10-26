# The same contract is run against Rails and the emitted exception runtime.
ParseFailure = ActionDispatch::Http::Parameters::ParseError
class DerivedParseFailure < ActionDispatch::Http::Parameters::ParseError; end
class MessageValue
  def to_s
    "message object"
  end
end

def check_parse_message(error, expected)
  raise "message #{error.message.inspect}, expected #{expected.inspect}" unless error.message == expected
end

# A helper call must see the caller's active exception; the default is evaluated
# on each invocation, including after an inner rescue restores the outer $!.
def make_parse_error(receiver, constructor, *arguments)
  receiver.public_send(constructor, *arguments)
end

checks = 0
[ActionDispatch::Http::Parameters::ParseError, ::ActionDispatch::Http::Parameters::ParseError,
 ParseFailure, DerivedParseFailure].each do |receiver|
  [:new, :exception].each do |constructor|
    # Omission outside rescue is deliberately different from explicit nil:
    # Rails calls $!.message, so it raises NoMethodError when $! is nil.
    begin
      make_parse_error(receiver, constructor)
      raise "missing active exception should fail"
    rescue NoMethodError => error
      raise "wrong missing method" unless error.name == :message
      checks += 1
    end

    ["explicit", "", nil, false, :symbol, 17, MessageValue.new].each do |message|
      expected = message.nil? ? receiver.name : message.to_s
      check_parse_message(make_parse_error(receiver, constructor, message), expected)
      checks += 1
      begin
        raise ArgumentError, "ignored default"
      rescue ArgumentError
        check_parse_message(make_parse_error(receiver, constructor, message), expected)
        checks += 1
      end
    end

    begin
      raise ArgumentError, "outer"
    rescue ArgumentError => outer
      check_parse_message(make_parse_error(receiver, constructor), "outer")
      checks += 1
      begin
        raise KeyError, "inner"
      rescue KeyError
        check_parse_message(make_parse_error(receiver, constructor), "inner")
        checks += 1
      end
      check_parse_message(make_parse_error(receiver, constructor), "outer")
      raise "active error was changed" unless $!.equal?(outer)
      checks += 1
    end

    # The default must use #message, including its polymorphic behavior.
    special = ArgumentError.new("stored")
    def special.message
      "overridden message"
    end
    begin
      raise special
    rescue ArgumentError
      check_parse_message(make_parse_error(receiver, constructor), "overridden message")
      checks += 1
    end
  end

  [:raise_class, :fail_class, :raise_new, :raise_exception, :explicit, :explicit_nil].each do |form|
    begin
      begin
        raise ArgumentError, "wrapped"
      rescue ArgumentError
        case form
        when :raise_class then raise receiver
        when :fail_class then fail receiver
        when :raise_new then raise receiver.new
        when :raise_exception then raise receiver.exception
        when :explicit then raise receiver, "replacement"
        when :explicit_nil then raise receiver, nil
        end
      end
    rescue receiver => error
      expected = case form
                 when :explicit then "replacement"
                 when :explicit_nil then receiver.name
                 else "wrapped"
                 end
      check_parse_message(error, expected)
      raise "lost cause" unless error.cause.is_a?(ArgumentError) && error.cause.message == "wrapped"
      checks += 1
    end
  end

  original = receiver.new("existing")
  raise "instance exception identity" unless original.exception.equal?(original)
  check_parse_message(original.exception("copy"), "copy")
  check_parse_message(original, "existing")
  checks += 3
end
puts "ParseError contract: #{checks} checks passed"
