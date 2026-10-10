//! `X = Struct.new(…, keyword_init: true)` in a class or module body
//! becomes the class `<owner>::X`. A shape the synthesis cannot write as
//! plain Ruby stays the constant it was, so it keeps reporting instead
//! of vanishing.

use roundhouse::ingest::ingest_library_classes;

fn names_and_constants(source: &str) -> (Vec<String>, Vec<String>) {
    let classes = ingest_library_classes(source.as_bytes(), "probe.rb").expect("ingest");
    let names = classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
    let constants = classes.iter().flat_map(|c| c.constants.iter().map(|(n, _)| n.as_str().to_string())).collect();
    (names, constants)
}

#[test]
fn a_struct_constant_becomes_its_class_after_the_owner() {
    let (names, constants) = names_and_constants(
        "module Billing\n  Line = Struct.new(:sku, :qty, keyword_init: true)\n  def self.lines = []\nend\n\nclass Renderer\n  Result = Struct.new(:html, keyword_init: true) do\n    def empty? = html.nil?\n  end\n  LIMIT = 3\nend\n",
    );
    assert_eq!(names, ["Renderer", "Renderer::Result", "Billing", "Billing::Line"]);
    assert_eq!(constants, ["LIMIT"]);
}

#[test]
fn a_struct_the_synthesis_cannot_write_stays_a_constant() {
    let (names, constants) = names_and_constants(
        "class Service\n  Asked = Struct.new(:success?, keyword_init: true)\n  Reserved = Struct.new(:end, keyword_init: true)\n  Splat = Struct.new(*FIELDS, keyword_init: true)\n  Loud = Struct.new(:a, keyword_init: true) do\n    attr_accessor :b\n  end\n  Strict = Struct.new(:a, keyword_init: false)\n  Either = Struct.new(:a, :b)\nend\n",
    );
    assert_eq!(names, ["Service"]);
    // `Either` takes `Either.new(1, 2)` and, since Ruby 3.2, `Either.new(a: 1, b: 2)`: no one constructor answers both.
    assert_eq!(constants, ["Asked", "Reserved", "Splat", "Loud", "Strict", "Either"]);
}
