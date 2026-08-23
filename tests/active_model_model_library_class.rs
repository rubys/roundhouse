//! `include ActiveModel::Model` on a class ingest sees as a LIBRARY
//! class, not a tableless model.
//!
//! The tableless-model path (`ingest::library_class::classify_class_file`
//! → `ClassKind::Model`) only looks at `app/models/`. campfire reopens
//! `ActionText::Attachment::OpengraphEmbed` from `lib/rails_ext/`, so it
//! is a library class no matter what it includes, and the `include`
//! survived into the emit — where `ActiveModel` is defined nowhere and
//! the class raised `NameError` at load time. That was one of the six
//! constants blocking a stub-free campfire boot.
//!
//! What the pass owes the class is the three methods the module
//! actually supplies at our call sites: the attribute-hash constructor,
//! `valid?`, and `persisted?`. What it owes the reader is a refusal,
//! ledgered, when it cannot supply them honestly.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(lib_src: &str) -> (String, Vec<String>) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/embed.rb"), lib_src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let residue = diags
        .iter()
        .filter(|d| d.message.contains("ActiveModel::Model"))
        .map(|d| d.message.clone())
        .collect();
    (out, residue)
}

const EMBED: &str = r#"class OpengraphEmbed
  include ActiveModel::Model

  attr_accessor :href, :url, :filename, :description

  def to_partial_path
    "action_text/attachables/opengraph_embed"
  end
end
"#;

#[test]
fn the_include_is_replaced_by_what_it_supplies() {
    let (src, residue) = emitted(EMBED);
    // The module has no definition in any emitted tree, so an `include`
    // that survives is a load-time NameError, not a no-op.
    assert!(!src.contains("include ActiveModel::Model"), "{src}");
    assert!(src.contains("def initialize(attrs = {})"), "{src}");
    assert!(src.contains("@href = attrs[:href]"), "{src}");
    assert!(src.contains("@description = attrs[:description]"), "{src}");
    assert!(src.contains("def valid?"), "{src}");
    assert!(src.contains("def persisted?"), "{src}");
    assert!(residue.is_empty(), "{residue:?}");
}

/// The one failure mode worth refusing outright: a synthesized
/// `valid? => true` would answer *yes* for a record the app wrote rules
/// to reject. The include stays, and the ledger says why.
#[test]
fn a_declared_validation_declines_and_says_so() {
    let (src, residue) = emitted(
        r#"class OpengraphEmbed
  include ActiveModel::Model

  attr_accessor :href

  validates :href, presence: true
end
"#,
    );
    assert!(!src.contains("def valid?"), "{src}");
    assert_eq!(residue.len(), 1, "{residue:?}");
    assert!(residue[0].contains("declares validations"), "{residue:?}");
}

/// A hand-written constructor is the whole point of the class that
/// wrote one — Rails' would be overridden anyway. Declining keeps the
/// decision visible rather than dropping the include on a guess.
#[test]
fn a_hand_written_initialize_declines() {
    let (src, residue) = emitted(
        r#"class OpengraphEmbed
  include ActiveModel::Model

  attr_accessor :href

  def initialize(href)
    @href = href
  end
end
"#,
    );
    assert!(src.contains("def initialize(href)"), "{src}");
    assert!(!src.contains("def initialize(attrs"), "{src}");
    assert_eq!(residue.len(), 1, "{residue:?}");
    assert!(residue[0].contains("its own initialize"), "{residue:?}");
}
