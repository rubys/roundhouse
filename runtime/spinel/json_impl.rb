# frozen_string_literal: true
# JSON — resolved by each tree's own stack. The scaffold boot.rb's
# `require_relative "runtime/json_impl"` lands here uniformly; this file
# loads the real implementation, with the dump facade below on Spinel:
#
# - spinel: the compiler's bundled `json` spin package
#   (packages/json — typed native binding to lib/sp_json.c;
#   `native_func :parse, [:string], :any` etc.), activated by the
#   plain `require`.
# - CRuby / JRuby: the stdlib json.
#
# NOT NAMED `json.rb`, and that is the whole point. A shim that
# DELEGATES to a library must not share the library's name: this file
# is reached by `require_relative`, which registers its absolute path
# in `$LOADED_FEATURES` before the body runs, so if it were `json.rb`
# and `runtime/` were on `$LOAD_PATH`, the `require "json"` below would
# resolve back to THIS file, find it already loading, return false, and
# the real JSON would never load. Every later `JSON.generate` then dies
# with `uninitialized constant JSON` — and `Base64.strict_encode64(
# JSON.generate(stream))` in `turbo_stream_from` is the first casualty.
#
# That is not hypothetical: the emitted Rakefile puts `runtime/` on the
# load path (`t.libs << "runtime"`), and `project.rs`'s BUNDLED table
# WRITES `require "json"` into emitted app and test files that name the
# constant. Both paths need the bare name to reach the real library.
# `runtime/base64.rb` and `runtime/erb.rb` may keep their library names
# because they DEFINE those modules rather than delegating.
#
# This file used to be a hand-rolled `JSON.generate` shim (String
# input only) from before spinel bundled the package — lobsters'
# extras (keybase/github/twitter/diff_bot) call `JSON.parse`, which
# the shim deliberately omitted. Both libraries handle turbo_stream_from's
# `JSON.generate("articles")` → `"\"articles\""`.
require "json"

# The native package's dump binding accepts only the value. Keep the Ruby
# facade here so its IO/limit arguments are not discarded by that binding.
# CRuby and JRuby already provide this facade in the real json library.
if RUBY_ENGINE == "spinel"
  require "stringio"
  module JSON
    def self.dump(obj, anIO = nil, limit = nil, options = nil)
      if options.nil?
        if limit.nil? && anIO.is_a?(Hash)
          options = anIO
          anIO = nil
        elsif limit.is_a?(Hash)
          options = limit
          limit = nil
        end
      end

      # Spinel's boxed literal-name respond_to? path ignores visibility.
      # Runtime names retain public-only instance checks; singleton queries
      # need the literal-name path to preserve class-object dispatch.
      to_io_method = :to_io
      write_method = :write
      unless anIO.nil?
        if anIO.is_a?(Module) ? anIO.respond_to?(:to_io) : anIO.respond_to?(to_io_method)
          anIO = anIO.to_io
        # Carried-C StringIO methods are callable but absent from Spinel's
        # respond_to? table. It is a writer, just like a Ruby-defined sink.
        elsif limit.nil? &&
            !(anIO.is_a?(Module) ? anIO.respond_to?(:write) : anIO.respond_to?(write_method)) &&
            !anIO.is_a?(StringIO)
          limit = anIO
          anIO = nil
        end
      end

      opts = { max_nesting: false, allow_nan: true }
      opts = opts.merge(max_nesting: limit) if limit
      # Match the supported MRI 3.4 line's bundled json 2.9.1. Later json
      # releases merge arbitrary generator options here; 2.9 accepts strict
      # only, and validates it after the destination's to_io conversion.
      if options
        unknown = options.keys.reject { |key| key == :strict }
        unless unknown.empty?
          label = unknown.length == 1 ? "unknown keyword: " : "unknown keywords: "
          raise ArgumentError, label + unknown.map { |key| key.inspect }.join(", ")
        end
        opts = opts.merge(options)
      end
      nesting = opts[:max_nesting]
      # Enforce the immediate-integer range json 2.9 accepts on a 64-bit
      # host. Other Numeric values keep the bundled encoder's behavior.
      nesting = 0 unless nesting.is_a?(Integer) &&
        nesting >= -4611686018427387904 && nesting <= 4611686018427387903
      begin
        strict = opts[:strict] ? true : false
        check_dump_value(obj, nesting, 0, strict) if nesting != 0 || strict
        json = generate(obj, opts)
        return json unless anIO
        anIO.write(json)
        anIO.flush
        anIO
      rescue JSON::NestingError
        raise ArgumentError, "exceed depth limit"
      end
    end

    def self.check_dump_value(value, limit, depth, strict)
      if value.is_a?(Array)
        raise ArgumentError, "exceed depth limit" if limit != 0 && depth >= limit
        value.each { |child| check_dump_value(child, limit, depth + 1, strict) }
      elsif value.is_a?(Hash)
        raise ArgumentError, "exceed depth limit" if limit != 0 && depth >= limit
        value.each_value { |child| check_dump_value(child, limit, depth + 1, strict) }
      elsif strict && !value.nil? && !value.is_a?(TrueClass) && !value.is_a?(FalseClass) &&
          !value.is_a?(String) && !value.is_a?(Integer) && !value.is_a?(Float)
        raise JSON::GeneratorError, value.class.to_s + " not allowed in JSON"
      end
      nil
    end
    private_class_method :check_dump_value
  end
end
