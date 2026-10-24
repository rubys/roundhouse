//! One `String#truncate` contract shared by the interpreted and native
//! lanes. ActiveSupport reopens `String` for it, which neither the
//! transpiled runtime nor spinel can host, so a String-typed call is
//! grounded to `ActiveSupport.truncate`. Before that grounding the
//! analyzer already typed the call String and `check` was quiet, while
//! the emitted program raised NoMethodError: a controller logging
//! `params.to_h.to_json.truncate(2000)` failed on the native binary.
//! The expected values are what Rails 8.1.4 answers for the same calls.
//! Value equality alone misses a borrowed return: even the no-truncation
//! and omission-only branches must return fresh mutable Strings.

pub const SOURCE: &str = r#"class StringTruncateProbe
  LONG = "Once upon a time in a world far far away".freeze
  OMISSION = "... (continued)".freeze

  def self.results
    text = LONG
    [
      text.truncate(27),
      text.truncate(27, separator: " "),
      text.truncate(27, separator: " ", omission: "~"),
      text.truncate(100),
      "And they found that many people were sleeping better.".truncate(25, omission: OMISSION),
      text.truncate(4, omission: OMISSION),
      "Supercalifragilistic".truncate(10, separator: " "),
      "żółć gęślą jaźń".truncate(8, omission: "…"),
      { "a" => "x" * 30 }.to_json.truncate(12),
    ]
  end
end
"#;

pub const ASSERTIONS: &str = r#"expected = [
  "Once upon a time in a wo...",
  "Once upon a time in a...",
  "Once upon a time in a~",
  "Once upon a time in a world far far away",
  "And they f... (continued)",
  "... (continued)",
  "Superca...",
  "żółć gę…",
  "{\"a\":\"xxx...",
]
actual = StringTruncateProbe.results
raise "String#truncate parity: #{actual.inspect}" unless actual == expected

source = StringTruncateProbe::LONG
omission = StringTruncateProbe::OMISSION
raise "ownership witness needs frozen inputs" unless source.frozen? && omission.frozen?
unchanged = actual[3]
omission_only = actual[5]
raise "no-truncation result aliases its receiver" if unchanged.equal?(source)
raise "omission-only result aliases its omission" if omission_only.equal?(omission)
raise "truncate returned frozen storage" if unchanged.frozen? || omission_only.frozen?
unchanged << "!"
omission_only << "?"
raise "truncate mutated its receiver" unless source == expected[3]
raise "truncate mutated its omission" unless omission == expected[5]
again = StringTruncateProbe.results
raise "result mutation poisoned a later call" unless again == expected
raise "a later call changed a retained result" unless unchanged == expected[3] + "!" && omission_only == expected[5] + "?"
puts "String truncate emitted contract passed"
"#;
