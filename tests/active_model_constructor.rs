//! `ActiveModel::Model`'s attribute-hash constructor, and the splat
//! form of `attr_accessor` that decides what it assigns.
//!
//! campfire's `Opengraph::Metadata` writes both:
//!
//! ```text
//! ATTRIBUTES = %i[ title url image description ]
//! attr_accessor *ATTRIBUTES
//! ```
//!
//! and is built exactly once, by `new attributes.merge(…)` in its own
//! `from_url`. Neither half worked: the splat expanded to nothing so the
//! class emitted NO accessors, and with only Object's zero-arg
//! `initialize` the build was "wrong number of arguments (given 1,
//! expected 0)".
//!
//! `ActiveModel::Validations` is deliberately NOT enough — it brings
//! `valid?`/`errors` and no constructor, which is why campfire's
//! `Opengraph::Location` (Validations + its own one-arg `initialize`)
//! must keep the constructor it wrote.

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

fn emitted(model_src: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n",
        ),
        ("app/models/card.rb", model_src),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("card.rb"))
        .expect("no card.rb emitted")
        .content
        .clone()
}

const SPLAT_MODEL: &str = r#"class Card
  include ActiveModel::Model

  ATTRIBUTES = %i[ title url ]
  attr_accessor *ATTRIBUTES
end
"#;

#[test]
fn attr_accessor_expands_a_splatted_constant_array() {
    let src = emitted(SPLAT_MODEL);
    assert!(src.contains("def title"), "{src}");
    assert!(src.contains("def title=(value)"), "{src}");
    assert!(src.contains("def url"), "{src}");
    assert!(src.contains("def url=(value)"), "{src}");
}

#[test]
fn active_model_model_gains_the_attribute_hash_constructor() {
    let src = emitted(SPLAT_MODEL);
    assert!(src.contains("def initialize(attrs = {})"), "{src}");
    assert!(src.contains("@title = attrs[:title]"), "{src}");
    assert!(src.contains("@url = attrs[:url]"), "{src}");
}

/// `ActiveModel::Validations` brings `valid?`/`errors` and NO
/// constructor — a class that includes only it, and writes its own
/// `initialize`, must keep the one it wrote.
#[test]
fn a_hand_written_initialize_is_never_replaced() {
    let src = emitted(
        r#"class Card
  include ActiveModel::Validations

  attr_accessor :url

  def initialize(url)
    @url = url
  end
end
"#,
    );
    assert!(src.contains("def initialize(url)"), "{src}");
    assert!(!src.contains("def initialize(attrs"), "{src}");
}
