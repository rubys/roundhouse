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

/// Ruby's last-definition-wins, at the spelling campfire's
/// `Opengraph::Location` uses: `attr_accessor :parsed_url` at the top of
/// the class and a memoizing `def parsed_url` further down. The `def`
/// REPLACES the accessor — emitting both kept the synthesized
/// `def parsed_url; @parsed_url; end` and lost the app's body, so the
/// ivar was never written and the reader answered nil for every
/// instance. That failed `validate_url` on every URL and made `valid?`
/// false throughout the subsystem: 9 of its own tests, none of which
/// named a missing method.
///
/// The same hole also surfaced as Spinel `error[ivar_unresolved]:
/// @parsed_url has no known type` — the bare reader reads an ivar no
/// typed write ever seeded. Keep the memo body, and keep analyze quiet
/// on that ivar for this pattern.
#[test]
fn a_def_replaces_the_accessor_it_shadows() {
    let src = emitted(
        r#"class Card
  include ActiveModel::Validations

  attr_accessor :url, :parsed_url

  def initialize(url)
    @url = url
  end

  private
    def parsed_url
      return @parsed_url if defined? @parsed_url
      @parsed_url = URI.parse(url)
    end
end
"#,
    );
    // The app's own body is what survives...
    assert!(src.contains("@parsed_url = URI.parse(url)"), "{src}");
    // ...as the ONLY reader of that name — not beside a synthesized one.
    assert_eq!(src.matches("def parsed_url\n").count(), 1, "{src}");
    // The writer half is unshadowed, so `attr_accessor` still supplies it.
    assert!(src.contains("def parsed_url=(value)"), "{src}");
    // And an accessor nothing shadows keeps both halves.
    assert!(src.contains("def url\n"), "{src}");
    assert!(src.contains("def url=(value)"), "{src}");
    // defined?-memo lowering must still rewrite the surviving body.
    assert!(
        src.contains("@parsed_url_defined") || src.contains("defined?"),
        "expected defined?-memo flag or guard in emitted body:\n{src}"
    );
}

#[test]
fn attr_then_defined_ivar_memo_has_no_ivar_unresolved() {
    use roundhouse::analyze::{diagnose, DiagnosticKind};
    use roundhouse::diagnostic::Severity;
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n",
        ),
        (
            "app/models/card.rb",
            r#"class Card
  include ActiveModel::Validations

  attr_accessor :url, :parsed_url

  def initialize(url)
    @url = url
  end

  private
    def parsed_url
      return @parsed_url if defined? @parsed_url
      @parsed_url = URI.parse(url) rescue nil
    end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let unresolved: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::IvarUnresolved { ref name } if name.as_str() == "parsed_url"
            ) && d.severity == Severity::Error
        })
        .collect();
    assert!(
        unresolved.is_empty(),
        "expected no error[ivar_unresolved] on @parsed_url, got:\n{:#?}",
        unresolved
    );
}

/// A body `def` over a schema column's reader is the one Rails calls:
/// ActiveRecord generates the column's accessors in a module the class
/// includes, so the class's own definition wins (`push_user_methods`).
#[test]
fn a_schema_column_reader_yields_to_a_body_def() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "cards", force: :cascade do |t|
    t.string "title", null: false
  end
end
"#,
        ),
        (
            "app/models/card.rb",
            r#"class Card < ApplicationRecord
  def title
    "override"
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("card.rb"))
        .expect("no card.rb emitted")
        .content
        .clone();
    assert!(
        src.contains("\"override\""),
        "the body def title must win over the column reader; got:\n{src}"
    );
    assert!(
        src.contains("def title"),
        "expected a title reader to remain:\n{src}"
    );
}
