//! `Array#fetch` and the `Array[...]` / `Hash[...]` constructors type in
//! the analyzer; the emitted Ruby runs them as Ruby's own.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"class Lists
  #: (Array[Integer]) -> Integer
  def self.first_of(list) = list.fetch(0)

  #: (Array[Integer]) -> Integer
  def self.default_of(list) = list.fetch(9, -1)

  #: (Array[Integer]) -> Integer
  def self.block_of(list) = list.fetch(9) { |index| index * 2 }

  #: -> Array[Integer]
  def self.built = Array[1, 2]

  #: -> Hash[Integer, Integer]
  def self.paired = Hash[[[1, 2]]]
end
"#;

#[test]
fn fetch_and_the_container_constructors_run() {
    emit_and_run::real_blog()
        .write("app/services/lists.rb", LIB)
        .run_ruby(
            r#"raise "fetch" unless Lists.first_of([7, 8]) == 7
raise "fetch default" unless Lists.default_of([7, 8]) == -1
raise "fetch block" unless Lists.block_of([7, 8]) == 18
raise "Array[]" unless Lists.built == [1, 2]
raise "Hash[]" unless Lists.paired == { 1 => 2 }
"#,
        )
        .assert_passes();
}
