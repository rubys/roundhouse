//! `class Page < Array` → Object wrapping `@elements`.
//!
//! Spinel refuses Array subclasses (`refuse_builtin_subclass`); Roundhouse
//! rewrites them at ingest into the wrap Spinel's refusal message asks
//! for. See `ingest::library_class::try_wrap_array_subclass`.

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

#[test]
fn array_subclass_emits_as_elements_wrapper() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
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
        .expect("Page library class");
    assert!(
        page.parent.is_none(),
        "Array superclass must be cleared, got {:?}",
        page.parent
    );
    let names: Vec<&str> = page
        .methods
        .iter()
        .filter(|m| m.receiver == roundhouse::dialect::MethodReceiver::Instance)
        .map(|m| m.name.as_str())
        .collect();
    for required in [
        "to_a", "to_ary", "each", "+", "any?", "first", "last", "map", "count", "select",
        "include?", "[]", "loaded?",
    ] {
        assert!(
            names.iter().any(|n| *n == required),
            "missing `{required}` on wrapped Page; have {names:?}"
        );
    }

    // Support classes land through `emit_library` (project assembly),
    // not the model-only `emit_spinel` slice.
    let files = ruby::emit_library(&app);
    let paths: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
    let src = files
        .iter()
        .find(|f| {
            f.content.contains("def loaded?") && f.path.extension().is_some_and(|e| e == "rb")
        })
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.is_empty(),
        "expected an emitted Page .rb; files were:\n{}",
        paths.join("\n")
    );
    assert!(
        !src.contains("< Array"),
        "emit must not subclass Array:\n{src}"
    );
    assert!(
        src.contains("@elements"),
        "emit must wrap records in @elements:\n{src}"
    );
    assert!(
        !src.contains("super(records)"),
        "super(records) must become @elements = records:\n{src}"
    );
    assert!(
        src.contains("first(*args)") && src.contains("@elements.first(*args)"),
        "first must splat-forward so first(n) works:\n{src}"
    );
    assert!(
        src.contains("def map(*args)") && src.contains("@elements.map(*args)"),
        "map must forward with optional block:\n{src}"
    );
    assert!(
        src.contains("def any?(*args)") && src.contains("def count(*args)"),
        "any?/count must accept a block via rest+block_given?:\n{src}"
    );
    // Honesty ledger: the wrap is not Array. `is_a?(Array)` stays false;
    // we do not override kind_of?/is_a?. Spinel #7584 is the real subclass.
    assert!(
        !src.contains("def is_a?") && !src.contains("def kind_of?"),
        "wrap must not fake Array identity:\n{src}"
    );
}

#[test]
fn bare_super_in_initialize_forwards_first_positional() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def initialize(records, relation)
      super
      @relation = relation
    end

    def first
      super
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.content.contains("@elements") && f.content.contains("def initialize"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(!src.is_empty(), "expected emitted Page with @elements");
    assert!(
        src.contains("@elements = records"),
        "bare super in initialize must forward first positional:\n{src}"
    );
    // Pure-`super` `first` is dropped so the synthesized forward wins —
    // keeping `def first; super; end` after clearing the Array parent
    // would raise at runtime.
    assert!(
        src.contains("@elements.first"),
        "pure-super first override must yield synthesized @elements.first:\n{src}"
    );
    let has_dead_super_first = src.split("def first").skip(1).any(|chunk| {
        let body = chunk.split("def ").next().unwrap_or("");
        body.contains("super") && !body.contains("@elements")
    });
    assert!(
        !has_dead_super_first,
        "pure-super first must not survive as dead super:\n{src}"
    );
}

#[test]
fn size_fill_super_keeps_array_parent() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def initialize
      super(3, :item)
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
        .expect("Page library class");
    assert!(
        page.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "size+fill super must keep Array parent, got {:?}",
        page.parent
    );
}

#[test]
fn integer_size_super_keeps_array_parent() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def initialize
      super(3)
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
        .expect("Page library class");
    assert!(
        page.parent
            .as_ref()
            .is_some_and(|p| p.0.as_str() == "Array"),
        "super(3) size form must keep Array parent, got {:?}",
        page.parent
    );
}

#[test]
fn empty_array_subclass_seeds_elements() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.content.contains("class Page") && f.content.contains("@elements"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("@elements = []") || src.contains("@elements=[]"),
        "empty Array subclass must seed @elements = []:\n{src}"
    );
    assert!(
        src.contains("block_given?"),
        "each/all? must branch on block_given?:\n{src}"
    );
}

#[test]
fn pure_super_first_n_yields_splat_forward() {
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
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def initialize(records)
      super(records)
    end

    def first(n)
      super(n)
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.content.contains("@elements") && f.content.contains("def first"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("def first(*args)"),
        "pure-super first(n) must yield splat forward:\n{src}"
    );
    let has_dead_super_first = src.split("def first").skip(1).any(|chunk| {
        let body = chunk.split("def ").next().unwrap_or("");
        body.contains("super") && !body.contains("@elements")
    });
    assert!(
        !has_dead_super_first,
        "pure-super first(n) must not survive as dead super:\n{src}"
    );
}

#[test]
fn colon_colon_array_parent_wraps() {
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
            r#"class Widget < ApplicationRecord
end
"#,
        ),
        (
            "app/models/tagged_list.rb",
            r#"class TaggedList < ::Array
  def initialize(records)
    super(records)
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let tagged = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "TaggedList")
        .expect("TaggedList library class");
    assert!(
        tagged.parent.is_none(),
        "::Array superclass must wrap, got {:?}",
        tagged.parent
    );
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.content.contains("class TaggedList") && f.content.contains("@elements"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.is_empty() && !src.contains("< Array") && !src.contains("< ::Array"),
        "TaggedList < ::Array must emit as Object wrap:\n{src}"
    );
}

#[test]
fn non_page_array_subclass_wraps() {
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
            r#"class Widget < ApplicationRecord
end
"#,
        ),
        (
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
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let bag = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Bag")
        .expect("Bag library class");
    assert!(
        bag.parent.is_none(),
        "non-Page Array subclass must wrap, got {:?}",
        bag.parent
    );
    let names: Vec<&str> = bag
        .methods
        .iter()
        .filter(|m| m.receiver == roundhouse::dialect::MethodReceiver::Instance)
        .map(|m| m.name.as_str())
        .collect();
    assert!(
        names.iter().any(|n| *n == "label") && names.iter().any(|n| *n == "to_a"),
        "Bag must keep user methods and synth protocol; have {names:?}"
    );
}
