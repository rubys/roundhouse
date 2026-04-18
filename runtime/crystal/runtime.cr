# Roundhouse Crystal runtime.
#
# Hand-written Crystal shipped alongside each generated app. The Crystal
# emitter copies this file verbatim into the generated project as
# `src/runtime.cr`, so generated code can reference
# `Roundhouse::ValidationError` et al.  Mirrors `runtime/rust/runtime.rs`
# — same per-target posture: minimal surface, with each new lowering
# adding exactly what it needs.

module Roundhouse
  # A single validation failure produced by a model's generated
  # `validate` method. Carries the attribute name and a human-readable
  # message; `full_message` composes them into a Rails-compatible
  # display string (`"Title can't be blank"`).
  class ValidationError
    getter field : String
    getter message : String

    def initialize(@field : String, @message : String)
    end

    # Rails-compatible display form: capitalize the field name, replace
    # underscores with spaces, prepend to the message.
    # `ValidationError.new("post_id", "can't be blank")` becomes
    # `"Post id can't be blank"`.
    def full_message : String
      label = @field.gsub('_', ' ')
      label = label.capitalize
      "#{label} #{@message}"
    end
  end
end
