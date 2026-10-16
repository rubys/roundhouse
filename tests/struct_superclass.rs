//! `class Image < Struct.new(:asset_path, :width, :height)` — a
//! superclass EXPRESSION.
//!
//! `constant_path_of` has no answer for a call node, so the parent came
//! back `None` and the class emitted as a bare `class Image`. Its
//! `super(...)` then reached `BasicObject#initialize` and campfire died
//! at LOAD time with "wrong number of arguments (given 3, expected 0)"
//! — the fifth wall of a stub-free boot, and a drop with no diagnostic
//! to show for it.
//!
//! The anonymous struct gets a NAME instead: a sibling class carrying a
//! reader, a writer, and a positional constructor per member, which is
//! what makes the subclass's `super` resolve on every target rather
//! than needing each one to grow a struct notion.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(src: &str) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/sound.rb"), src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .map(|f| (f.path.display().to_string(), f.content))
        .collect()
}

const SOUND: &str = r#"class Sound
  class Image < Struct.new(:asset_path, :width, :height)
    def initialize(name:, width:, height:)
      super "sounds/#{name}", width, height
    end
  end
end
"#;

fn find<'a>(files: &'a [(String, String)], stem: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(stem))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| panic!("no {stem} in {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>()))
}

#[test]
fn the_superclass_expression_becomes_a_named_class() {
    let files = emitted(SOUND);
    // Both live in their parent's file now, so the assertion is on
    // ORDER rather than on a require between two files: the base has
    // to be defined when the `class X < Y` line runs, and that is the
    // requirement either layout has to meet.
    let sound = find(&files, "sound.rb");
    assert!(sound.contains("class Image < Sound::ImageStruct"), "{sound}");
    let base_at = sound.find("class ImageStruct").expect("the base is emitted");
    let sub_at = sound.find("class Image <").expect("the subclass is emitted");
    assert!(base_at < sub_at, "the base must come first:\n{sound}");
}

#[test]
fn the_named_base_carries_the_positional_constructor_super_calls() {
    let files = emitted(SOUND);
    let base = find(&files, "sound.rb");
    assert!(
        base.contains("def initialize(asset_path = nil, width = nil, height = nil)"),
        "{base}"
    );
    assert!(base.contains("@asset_path = asset_path"), "{base}");
    // Reader and writer per member, as `Struct` gives.
    assert!(base.contains("def width\n"), "{base}");
    assert!(base.contains("def width=(value)"), "{base}");
}

/// The keyword-init form means a DIFFERENT constructor, so recognizing
/// it as the positional one would build a class whose `new` takes the
/// wrong arguments. Declined — the parent stays unresolved, which is
/// the state that was already ledgered rather than a new wrong answer.
#[test]
fn the_keyword_init_form_is_not_claimed() {
    let files = emitted(
        r#"class Sound
  class Image < Struct.new(:asset_path, keyword_init: true)
  end
end
"#,
    );
    let image = find(&files, "sound.rb");
    assert!(!image.contains("ImageStruct"), "{image}");
    assert!(files.iter().all(|(p, _)| !p.ends_with("image_struct.rb")), "{files:?}");
}

fn diagnostics(files: &[(&str, &str)]) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    for (path, src) in files {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.message)
        .collect()
}

/// Since Ruby 3.2 a positional Struct also takes `new(x: 1, y: 2)`, which
/// the synthesized positional constructor would bind whole to `x`. That
/// call reports; positional and braced-Hash calls do not, and neither
/// does a subclass whose own `initialize` takes keywords.
#[test]
fn keyword_construction_of_a_positional_struct_reports() {
    let errors = diagnostics(&[
        ("app/models/point.rb", "class Point < Struct.new(:x, :y)\n  def sum\n    x + y\n  end\nend\n"),
        ("app/models/sound.rb", SOUND),
        (
            "app/models/maker.rb",
            "class Maker\n  def self.points\n    [Point.new(1, 2), Point.new({ x: 1, y: 2 }), Sound::Image.new(name: \"a\", width: 1, height: 2)]\n  end\n\n  def self.keyword_point\n    Point.new(x: 1, y: 2)\n  end\nend\n",
        ),
    ]);
    let keyword: Vec<_> = errors.iter().filter(|m| m.contains("Struct keyword construction")).collect();
    assert_eq!(keyword.len(), 1, "{errors:#?}");
}
