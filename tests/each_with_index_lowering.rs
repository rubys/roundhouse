//! `each.with_index` → `each_with_index` lowering.
//!
//! Writebook's `Positionable#move_to_position` emits
//! `all_to_move.each.with_index(1) { |item, index| item.update!(…) }`,
//! which Spinel AOT rejects when the block makes a keyword call that
//! closes over the index. The shared lowerer flattens the Enumerator
//! chain; this pin checks the emitted Ruby, not IR alone.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn emit_lib(src: &str, stem: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from(format!("app/models/{stem}.rb")), src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(&format!("{stem}.rb")))
        .unwrap_or_else(|| panic!("no {stem}.rb emitted"))
        .content
}

#[test]
fn each_with_index_offset_collapses_to_each_with_index() {
    let src = emit_lib(
        r#"class Mover
  def self.reposition(items, before, gap)
    items.each.with_index(1) do |item, index|
      item.update!(score: before + index * gap)
    end
  end
end
"#,
        "mover",
    );
    assert!(
        src.contains("each_with_index"),
        "each.with_index(1) should become each_with_index:\n{src}"
    );
    assert!(
        !src.contains("each.with_index"),
        "Enumerator chain must not survive:\n{src}"
    );
    assert!(
        src.contains("__with_index_i") && src.contains("index ="),
        "nonzero offset should bind index = __with_index_i + offset:\n{src}"
    );
}

#[test]
fn each_with_index_offset_does_not_shadow_existing_temp_name() {
    let src = emit_lib(
        r#"class Mover
  def self.reposition(items, __with_index_i)
    items.each.with_index(1) do |item, index|
      item.update!(score: before_plus(__with_index_i, index))
    end
  end

  def self.before_plus(captured, index)
    captured + index
  end
end
"#,
        "mover",
    );
    assert!(
        src.contains("each_with_index"),
        "still flatten each.with_index:\n{src}"
    );
    assert!(
        src.contains("__with_index_i1") || !src.contains("|item, __with_index_i|"),
        "injected index temp must not reuse the captured local name:\n{src}"
    );
}

#[test]
fn each_with_index_without_offset_needs_no_binding() {
    let src = emit_lib(
        r#"class Mover
  def self.reposition(items)
    items.each.with_index do |item, index|
      item.update!(score: index)
    end
  end
end
"#,
        "mover",
    );
    assert!(
        src.contains("each_with_index"),
        "bare each.with_index should become each_with_index:\n{src}"
    );
    assert!(
        !src.contains("__with_index_i"),
        "zero-start needs no synthetic index binding:\n{src}"
    );
}

#[test]
fn map_with_index_is_left_alone() {
    let src = emit_lib(
        r##"class Mapper
  def self.label(items)
    items.map.with_index(1) { |s, i| "#{i}:#{s}" }
  end
end
"##,
        "mapper",
    );
    assert!(
        src.contains("map.with_index") || src.contains(".with_index"),
        "map.with_index must not be rewritten to each_with_index:\n{src}"
    );
    assert!(
        !src.contains("each_with_index"),
        "map.with_index must not become each_with_index:\n{src}"
    );
}

#[test]
fn each_with_index_dynamic_offset_is_left_alone() {
    let src = emit_lib(
        r#"class Mover
  def self.reposition(items, start)
    items.each.with_index(start) do |item, index|
      item.update!(score: index)
    end
  end
end
"#,
        "mover",
    );
    assert!(
        src.contains("each.with_index") || src.contains(".with_index"),
        "non-literal offset must keep each.with_index (eval once):\n{src}"
    );
    assert!(
        !src.contains("each_with_index"),
        "dynamic offset must not materialize into each_with_index:\n{src}"
    );
}

#[test]
fn each_with_index_method_ref_offset_is_left_alone() {
    let src = emit_lib(
        r#"class Mover
  def self.touch(item, index)
    item.update!(score: index)
  end

  def self.reposition(items)
    items.each.with_index(1, &method(:touch))
  end
end
"#,
        "mover",
    );
    assert!(
        src.contains("with_index"),
        "method-ref block must keep each.with_index so offset is not dropped:\n{src}"
    );
    assert!(
        !src.contains("each_with_index"),
        "method-ref + nonzero offset must not rewrite (would drop offset):\n{src}"
    );
}
