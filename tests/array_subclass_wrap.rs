//! Wrappable `class X < Array` / `::Array` → Object wrapping `@elements`.
//!
//! Spinel refuses Array subclasses (`refuse_builtin_subclass`); Roundhouse
//! rewrites them at ingest. See `ingest::array_subclass_wrap`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// Minimal scaffold so a library class under `app/models/` is ingested.
fn library_app(class_file: &str, class_src: &str) -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\nend\n",
        ),
        (class_file, class_src),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn library_rb(app: &roundhouse::App, needle: &str) -> String {
    ruby::emit_library(app)
        .into_iter()
        .find(|f| f.content.contains(needle) && f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .unwrap_or_default()
}

fn has_dead_super_named(src: &str, name: &str) -> bool {
    let marker = format!("def {name}");
    src.split(&marker).skip(1).any(|chunk| {
        let body = chunk.split("def ").next().unwrap_or("");
        body.contains("super") && !body.contains("@elements")
    })
}

#[test]
fn non_page_array_subclass_wraps() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(records)
    super(records)
  end

  def label
    "bag"
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(bag.parent.is_none(), "got {:?}", bag.parent);
    let names: Vec<&str> = bag
        .methods
        .iter()
        .filter(|m| m.receiver == roundhouse::dialect::MethodReceiver::Instance)
        .map(|m| m.name.as_str())
        .collect();
    for required in [
        "to_a", "to_ary", "each", "+", "any?", "first", "last", "map", "count", "select",
        "include?", "[]", "label",
    ] {
        assert!(
            names.iter().any(|n| *n == required),
            "missing `{required}`; have {names:?}"
        );
    }
    let src = library_rb(&app, "def label");
    assert!(!src.is_empty(), "expected emitted Bag");
    assert!(!src.contains("< Array"), "must not subclass Array:\n{src}");
    assert!(src.contains("@elements"), "must wrap in @elements:\n{src}");
    assert!(
        !src.contains("super(records)"),
        "super(records) → @elements = records:\n{src}"
    );
    assert!(
        src.contains("first(*args)") && src.contains("@elements.first(*args)"),
        "first must splat-forward:\n{src}"
    );
    assert!(
        src.contains("def map(*args)") && src.contains("@elements.map(*args)"),
        "map must forward with optional block:\n{src}"
    );
    assert!(
        src.contains("def any?(*args)") && src.contains("def count(*args)"),
        "any?/count must accept a block:\n{src}"
    );
    // Honesty ledger: wrap is not Array identity.
    assert!(
        !src.contains("def is_a?") && !src.contains("def kind_of?"),
        "wrap must not fake Array identity:\n{src}"
    );
}

#[test]
fn colon_colon_array_parent_wraps() {
    let app = library_app(
        "app/models/tagged_list.rb",
        r#"class TaggedList < ::Array
  def initialize(records)
    super(records)
  end
end
"#,
    );
    let tagged = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "TaggedList")
        .expect("TaggedList");
    assert!(tagged.parent.is_none(), "got {:?}", tagged.parent);
    let src = library_rb(&app, "class TaggedList");
    assert!(
        !src.is_empty() && !src.contains("< Array") && !src.contains("< ::Array"),
        "TaggedList < ::Array must emit as Object wrap:\n{src}"
    );
    assert!(src.contains("@elements"), "{src}");
}

#[test]
fn campfire_page_array_subclass_wraps() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
end
"#,
        ),
        ("app/models/message.rb", "class Message < ApplicationRecord
end
"),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def self.load(relation, direction, size)
      new(relation.first(size), relation)
    end

    def initialize(records, relation)
      super(records)
      @relation = relation
    end

    def loaded?
      true
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let page = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Message::Pagination::Page")
        .expect("Page");
    assert!(page.parent.is_none(), "got {:?}", page.parent);
    let src = library_rb(&app, "def loaded?");
    assert!(!src.contains("< Array") && src.contains("@elements"), "{src}");
}

#[test]
fn bare_super_in_initialize_forwards_first_positional() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(records, tag)
    super
    @tag = tag
  end

  def first
    super
  end
end
"#,
    );
    let src = library_rb(&app, "@elements");
    assert!(
        src.contains("@elements = records"),
        "bare super must forward first positional:\n{src}"
    );
    assert!(
        src.contains("@elements.first") && !has_dead_super_named(&src, "first"),
        "pure-super first must yield synthesized forward:\n{src}"
    );
}

#[test]
fn bare_super_without_positional_keeps_array_parent() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize
    super
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(
        bag.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "bare super with no positional must keep Array parent, got {:?}",
        bag.parent
    );
}

#[test]
fn size_fill_super_keeps_array_parent() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize
    super(3, :item)
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(
        bag.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "got {:?}",
        bag.parent
    );
}

#[test]
fn integer_size_super_keeps_array_parent() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize
    super(3)
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(
        bag.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "got {:?}",
        bag.parent
    );
}

#[test]
fn empty_array_subclass_seeds_elements() {
    let app = library_app("app/models/bag.rb", "class Bag < Array\nend\n");
    let src = library_rb(&app, "class Bag");
    assert!(
        src.contains("@elements = []") || src.contains("@elements=[]"),
        "empty subclass must seed @elements = []:\n{src}"
    );
    assert!(src.contains("block_given?"), "each/all? need block_given?:\n{src}");
}

#[test]
fn initialize_without_super_seeds_elements() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(x)
    @x = x
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(bag.parent.is_none(), "got {:?}", bag.parent);
    let src = library_rb(&app, "class Bag");
    assert!(
        src.contains("@elements = []") || src.contains("@elements=[]"),
        "initialize without super must seed @elements = []:\n{src}"
    );
}

#[test]
fn index_forward_uses_splat() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(records)
    super(records)
  end
end
"#,
    );
    let src = library_rb(&app, "@elements");
    assert!(
        src.contains("def [](*args)")
            && (src.contains("@elements[*args]") || src.contains("@elements.[](*args)")),
        "[] must splat-forward for page[0, 2]:\n{src}"
    );
}

#[test]
fn decorated_super_on_protocol_keeps_array_parent() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(records)
    super(records)
  end

  def first
    r = super
    r
  end
end
"#,
    );
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag");
    assert!(
        bag.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "decorated super on protocol name must keep Array parent, got {:?}",
        bag.parent
    );
}

#[test]
fn pure_super_first_n_yields_splat_forward() {
    let app = library_app(
        "app/models/bag.rb",
        r#"class Bag < Array
  def initialize(records)
    super(records)
  end

  def first(n)
    super(n)
  end
end
"#,
    );
    let src = library_rb(&app, "def first");
    assert!(
        src.contains("def first(*args)") && !has_dead_super_named(&src, "first"),
        "pure-super first(n) must yield splat forward:\n{src}"
    );
}
