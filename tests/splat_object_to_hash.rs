//! `{ **attrs, k: v }` splats `attrs` through its `to_hash`. The ingest
//! desugar writes `attrs.merge({ k: v })`; for an object of an app class
//! (a log-attributes wrapper with `[]` and `to_hash`) the lowering reads it
//! through `to_hash` first: it raised NoMethodError for `merge`. An explicit
//! `merge` the class gets from an included module is left as written.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const WRAPPER: &str = r##"# frozen_string_literal: true

class LogAttributes
  attr_reader :log_attributes

  def initialize(attrs = {})
    @log_attributes = attrs
  end

  def [](key)
    @log_attributes[key]
  end

  def to_hash
    log_attributes
  end
end
"##;

const RESULT: &str = r##"# frozen_string_literal: true

module PolicyResult
  extend T::Sig
  extend T::Helpers
  interface!

  sig { abstract.returns(LogAttributes) }
  def log_attributes; end

  def event
    {
      **log_attributes,
      message: "#{log_attributes[:policy]}: done",
    }
  end

  class Completed
    include PolicyResult

    #: (log_attributes: LogAttributes) -> void
    def initialize(log_attributes:)
      @log_attributes = log_attributes
    end

    #: LogAttributes
    attr_reader(:log_attributes)
  end
end
"##;

#[test]
fn a_splatted_app_object_is_read_through_to_hash() {
    emit_and_run::empty_app()
        .write("app/models/log_attributes.rb", WRAPPER)
        .write("app/models/policy_result.rb", RESULT)
        .run_ruby(
            r#"
event = PolicyResult::Completed.new(log_attributes: LogAttributes.new({ policy: "alt" })).event
raise event.inspect unless event == { policy: "alt", message: "alt: done" }
puts "passed"
"#,
        )
        .assert_passes();
}

const MERGEABLE: &str = r##"# frozen_string_literal: true

module Mergeable
  #: (Hash[Symbol, untyped]) -> String
  def merge(other)
    "merged:#{other.keys.join(",")}"
  end
end
"##;

const MERGING: &str = r##"# frozen_string_literal: true

class MergingAttributes
  include Mergeable

  def to_hash
    { a: 1 }
  end

  #: () -> String
  def combined
    MergingAttributes.new.merge({ b: 2 })
  end
end
"##;

#[test]
fn a_merge_an_included_module_defines_is_called_as_written() {
    emit_and_run::empty_app()
        .write("app/models/concerns/mergeable.rb", MERGEABLE)
        .write("app/models/merging_attributes.rb", MERGING)
        .run_ruby(
            r#"
got = MergingAttributes.new.combined
raise got.inspect unless got == "merged:b"
puts "passed"
"#,
        )
        .assert_passes();
}
