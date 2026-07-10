# frozen_string_literal: true

# CRuby overlay: runtime encoder behind inline `render json: <expr>`
# (the controller rewrite lowers that to
# `render(ActionController::JsonRender.encode(v), content_type:
# "application/json")`).
#
# Mirrors Rails' to_json pipeline shape: `as_json` first (a model's
# custom serialization hook — lobsters' Story#as_json builds its API
# hash), then a structural walk that JSON-primitivizes the values.
# Time renders as iso8601(3), the ActiveSupport as_json format.
#
# Serializes its own JSON text (`emit_json`) rather than calling
# `JSON.generate`: the transpiled runtime ships a `JSON` shim module
# (runtime/json.rb, for targets without a json stdlib) whose `generate`
# shadows the gem's — routing through it here would `to_s` the payload
# into Ruby-inspect notation, not JSON.
#
# CRuby-only by nature (respond_to? dispatch); other targets surface
# `render json:` as an unresolved constant until their runtime grows
# an encoder.
module ActionController
  module JsonRender
    def self.encode(value)
      emit_json(jsonify(value))
    end

    def self.jsonify(value)
      # A custom as_json wins, whatever the object (Rails semantics).
      # Guard on arity-tolerant call: Rails' as_json takes an options
      # hash; model-emitted ones declare `(options = {})`.
      if !value.is_a?(Hash) && !value.is_a?(Array) && value.respond_to?(:as_json)
        return jsonify_structural(value.as_json)
      end
      jsonify_structural(value)
    end

    def self.jsonify_structural(value)
      case value
      when Hash
        out = {}
        value.each { |k, v| out[k.to_s] = jsonify(v) }
        out
      when Array
        value.map { |v| jsonify(v) }
      when Time
        value.iso8601(3)
      when String, Integer, Float, TrueClass, FalseClass, NilClass
        value
      when Symbol
        value.to_s
      else
        # Last resort: an object with no as_json — its to_s is more
        # useful in a payload than a crash. (ActiveRecord rows always
        # have accessors; this arm is for stray value objects.)
        value.to_s
      end
    end

    # JSON text from an already-jsonified tree (String/Numeric/bool/
    # nil/Array/Hash-with-string-keys only).
    def self.emit_json(value)
      case value
      when Hash
        "{" + value.map { |k, v| "#{emit_json(k.to_s)}:#{emit_json(v)}" }.join(",") + "}"
      when Array
        "[" + value.map { |v| emit_json(v) }.join(",") + "]"
      when String
        quote_json(value)
      when NilClass
        "null"
      else # Integer, Float, true, false
        value.to_s
      end
    end

    JSON_ESCAPES = {
      "\"" => "\\\"", "\\" => "\\\\", "\b" => "\\b", "\f" => "\\f",
      "\n" => "\\n", "\r" => "\\r", "\t" => "\\t"
    }.freeze

    def self.quote_json(s)
      escaped = s.gsub(/["\\\x00-\x1f]/) do |c|
        JSON_ESCAPES[c] || format("\\u%04x", c.ord)
      end
      "\"#{escaped}\""
    end
  end
end
