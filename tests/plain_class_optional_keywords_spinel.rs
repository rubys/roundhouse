//! #800 on the Spinel target: a plain library class's optional keyword
//! parameters, called with keywords, bind by name in the native binary.
//! `tests/plain_class_optional_keywords.rs` holds the CRuby oracle and
//! the refusals; this is the same program compiled by Spinel.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BUILDER: &str = r##"module Kwlab
  class Builder
    attr_reader :rows

    def initialize
      @rows = []
    end

    def field(name, type = nil, null: true, description: nil)
      [name.to_s, type.to_s, null.to_s, description.to_s].join("|")
    end

    def add(name, null: true)
      @rows << field(name, :string, null: null)
      @rows << self.field(name, description: "self")
      self
    end

    def self.column(name, limit: 8, comment: "")
      [name.to_s, limit.to_s, comment].join("|")
    end

    def self.report
      b = new
      [b.field(:a, :string, null: false), b.field(:b, :string, description: "kept"), b.field(:c),
       b.add(:d, null: false).rows.join(","), column(:e, comment: "c"), self.column(:f, limit: 2)]
    end
  end
end
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn optional_keywords_bind_by_name_in_the_spinel_binary() {
    let run = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", BUILDER)
        .run_spinel("Kwlab::Builder.report.each { |row| puts row }\n");
    run.assert_passes();
    assert_eq!(
        run.stdout,
        "a|string|false|\nb|string|true|kept\nc||true|\nd|string|false|,d||true|self\ne|8|c\nf|2|\n"
    );
}
