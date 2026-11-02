//! `filter_map { |w| cond && value }` keeps `value`'s truthy results
//! only: when `cond` is false the block answers `false`, and
//! `filter_map` drops it. The element type is `value`'s without nil,
//! not a union with the Bool the `&&`'s left arm can answer. With the
//! Bool in it, a method the elements reach was declared to take `bool`,
//! and Spinel's `--rbs .` refused the String it is passed.
//!
//! A block that answers a real Bool keeps `true` in the result, so its
//! element stays Bool. `map` and `select` are unchanged.
//!
//! Expected values are Ruby's (oracle `ruby-oracle.rb`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const NORMALIZER: &str = "class TagNormalizer\n  def normalize(str)\n    str.downcase\n  end\nend\n";

const NAME_NORMALIZER: &str = "class NameNormalizer\n  def normalize(str)\n    str.downcase\n  end\nend\n";

// The values come from parsed JSON, so the `&&`'s right arm is untyped
// and its left arm is Bool.
const ITEM_TAGS: &str = r##"class ItemTags
  def self.names(items)
    Array(items).filter_map { |item| item["type"] == "Hashtag" && item["name"]&.delete_prefix("#") }.map { |n| NameNormalizer.new.normalize(n) }
  end
end
"##;

const TAG_LIST: &str = r##"class TagList
  def self.names(text)
    text.to_s.split(" ").filter_map { |w| w.start_with?("#") && w.delete_prefix("#") }.map { |n| TagNormalizer.new.normalize(n) }
  end

  def self.bare(text)
    text.to_s.split(" ").filter_map { |w| w.start_with?("#") && w.delete_prefix("#") }
  end

  def self.skipping(text)
    text.to_s.split(" ").filter_map { |w| next w.size if w == "bar"; w.start_with?("#") && w.delete_prefix("#") }
  end

  def self.skipping_false(text)
    text.to_s.split(" ").filter_map { |w| next false if w == "bar"; w.start_with?("#") && w.delete_prefix("#") }
  end

  def self.skipping_true(text)
    text.to_s.split(" ").filter_map { |w| next true if w == "bar"; w.start_with?("#") && w.delete_prefix("#") }
  end

  def self.flags(text)
    text.to_s.split(" ").filter_map { |w| w.start_with?("#") }
  end

  def self.marked(text)
    text.to_s.split(" ").map { |w| w.start_with?("#") && w.delete_prefix("#") }
  end

  def self.tagged(text)
    text.to_s.split(" ").select { |w| w.start_with?("#") && w.size > 1 }
  end
end
"##;

// An app's own `filter_map` that answers its block's value is not
// Enumerable's: it drops nothing, so the `&&`'s false stays in its type,
// called on a receiver or on self.
const BASKET: &str = r##"class Basket
  def filter_map
    yield 1
  end

  def own(text)
    filter_map { |n| text.to_s.empty? && "x" }
  end

  def self.custom(text)
    Basket.new.filter_map { |n| text.to_s.empty? && "x" }
  end
end
"##;

const ASSERTIONS: &str = r##"
text = "#Foo bar #Baz"
items = [{ "type" => "Hashtag", "name" => "#Foo" }, { "type" => "Note", "name" => "x" }, { "type" => "Hashtag" }]
raise ItemTags.names(items).inspect unless ItemTags.names(items) == ["foo"]
got = [TagList.names(text), TagList.bare(text), TagList.skipping(text), TagList.skipping_false(text), TagList.skipping_true(text), TagList.flags(text), TagList.marked(text), TagList.tagged(text)]
want = [["foo", "baz"], ["Foo", "Baz"], ["Foo", 3, "Baz"], ["Foo", "Baz"], ["Foo", true, "Baz"], [true, true], ["Foo", false, "Baz"], ["#Foo", "#Baz"]]
raise got.inspect unless got == want
raise "custom" unless Basket.custom(text) == false && Basket.custom("") == "x" && Basket.new.own(text) == false
puts "filter_map passed"
"##;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/models/tag_normalizer.rb", NORMALIZER)
        .write("app/models/tag_list.rb", TAG_LIST)
        .write("app/models/name_normalizer.rb", NAME_NORMALIZER)
        .write("app/models/item_tags.rb", ITEM_TAGS)
        .write("app/models/basket.rb", BASKET)
}

fn sig_line(sig: &str, name: &str) -> String {
    sig.lines()
        .find(|l| l.trim_start().starts_with(&format!("def {name}:")))
        .unwrap_or_else(|| panic!("no `def {name}:` in:\n{sig}"))
        .trim()
        .to_string()
}

#[test]
fn a_filter_map_of_cond_and_value_yields_the_values_type() {
    let (emitted, errors) = example().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("read {p}: {e}"));
    let normalizer = read("app/models/tag_normalizer.rbs");
    let list = read("app/models/tag_list.rbs");
    let name_normalizer = read("app/models/name_normalizer.rbs");
    // An untyped value: no type to give the parameter, and not Bool.
    assert_eq!(sig_line(&name_normalizer, "normalize"), "def normalize: (untyped str) -> untyped");
    assert_eq!(sig_line(&normalizer, "normalize"), "def normalize: (String str) -> String");
    assert_eq!(sig_line(&list, "self.bare"), "def self.bare: (untyped text) -> Array[String]");
    assert_eq!(sig_line(&list, "self.names"), "def self.names: (untyped text) -> Array[String]");
    // A `next` value is an element too.
    assert_eq!(sig_line(&list, "self.skipping"), "def self.skipping: (untyped text) -> Array[(Integer | String)]");
    // `next false` is dropped like the `&&`'s false; `next true` is kept.
    assert_eq!(sig_line(&list, "self.skipping_false"), "def self.skipping_false: (untyped text) -> Array[String]");
    assert_eq!(sig_line(&list, "self.skipping_true"), "def self.skipping_true: (untyped text) -> Array[(bool | String)]");
    // A real Bool keeps its `true`s.
    assert_eq!(sig_line(&list, "self.flags"), "def self.flags: (untyped text) -> Array[bool]");
    // `map` and `select` are unchanged.
    assert_eq!(sig_line(&list, "self.marked"), "def self.marked: (untyped text) -> Array[(bool | String)]");
    assert_eq!(sig_line(&list, "self.tagged"), "def self.tagged: (untyped text) -> Array[String]");
    // An app's own block-value `filter_map` keeps the false.
    let basket = read("app/models/basket.rbs");
    assert_eq!(sig_line(&basket, "self.custom"), "def self.custom: (untyped text) -> (bool | String)");
    assert_eq!(sig_line(&basket, "own"), "def own: (untyped text) -> (bool | String)");
}

#[test]
fn a_filter_map_of_cond_and_value_runs_on_ruby() {
    let run = example().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("filter_map passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_filter_map_of_cond_and_value_runs_on_spinel_with_rbs() {
    let run = example().run_spinel_with_rbs(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("filter_map passed"), "{}", run.stdout);
}
