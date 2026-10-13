//! A model file is walked twice: once by the model ingester, and once
//! more by the plain library-class walk, just to pick up classes
//! nested inside the model's body (`app.rs`'s "classes nested in the
//! model's body are classes of their own"). That second walk doesn't
//! know about anything the model synthesizes — an `enum`-generated
//! predicate, say — so an `alias` naming one failed there even though
//! the model itself ingests fine. Worse, the hard `Err` from that
//! failure aborted the WHOLE file, including any genuinely nested
//! class it also contained.
//!
//! The fix: the second walk only looks at classes/modules actually
//! nested inside some other declaration, never at a file's top-level
//! class — which is the model itself, already ingested separately.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ModelBodyItem;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.integer "state", default: 0, null: false
  end
end
"#;

/// The spurious case: an `enum`-generated predicate aliased in the
/// same model body. The model walk already resolves this (the enum
/// desugars into `ModelBodyItem::Method` entries before the `alias`
/// statement is reached), so the model itself ingests fine — the gap
/// came only from the redundant library re-walk.
const WIDGET_WITH_ENUM_ALIAS: &str = r#"class Widget < ApplicationRecord
  enum :state, { idle: 0, accepted: 1 }
  alias enabled? accepted?
end
"#;

#[test]
fn enum_alias_in_a_model_body_records_no_gap_and_keeps_the_alias() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", WIDGET_WITH_ENUM_ALIAS),
    ]))
    .expect("the enum alias must not fail strict ingest");

    let model = app.models.iter().find(|m| m.name.0.as_str() == "Widget").expect("Widget model");
    // The alias itself is recorded as a typed body item at ingest, not
    // resolved yet — resolution moved to lowering (see
    // `ModelBodyItem::Alias`'s own doc). What this test is actually
    // about — the SECOND, redundant library walk no longer treats the
    // alias as a hard error — holds regardless: ingest above already
    // succeeded.
    assert!(
        model.body.iter().any(|item| matches!(item,
            ModelBodyItem::Alias { new, old, .. } if new.as_str() == "enabled?" && old.as_str() == "accepted?")),
        "{:?}", model.body
    );
    let lowered =
        roundhouse::lower::lower_model_to_library_class(model, &app.schema);
    let enabled = lowered
        .methods
        .iter()
        .find(|method| method.name.as_str() == "enabled?")
        .expect("the alias must still produce an `enabled?` method once lowered");
    let accepted = lowered
        .methods
        .iter()
        .find(|method| method.name.as_str() == "accepted?")
        .expect("the enum predicate `accepted?` must exist");
    assert_eq!(
        format!("{:?}", enabled.body), format!("{:?}", accepted.body),
        "the alias must copy the enum predicate's own body"
    );
}

/// A model file whose body ALSO contains a genuinely nested class
/// must still register that class as a library class, even though
/// the model itself has an `alias` the library walk alone couldn't
/// resolve.
#[test]
fn a_model_with_an_enum_alias_still_ingests_its_genuinely_nested_class() {
    let source = r#"class Widget < ApplicationRecord
  enum :state, { idle: 0, accepted: 1 }
  alias enabled? accepted?

  class Badge
    def self.describe
      "nested"
    end
  end
end
"#;
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", source),
    ]))
    .expect("must ingest despite the enum alias");

    let nested = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Widget::Badge")
        .expect("the nested class must still be registered as a library class");
    assert!(
        nested.methods.iter().any(|m| m.name.as_str() == "describe"),
        "the nested class's own method must survive: {nested:?}"
    );

    // The model's own top-level class must NOT also show up as a
    // library class under its own (unqualified) name — the second
    // walk exists only to find nested classes.
    assert!(
        app.library_classes.iter().all(|c| c.name.0.as_str() != "Widget"),
        "the model's own class must not be duplicated as a library class: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}

/// Same gap, the OTHER model-file site: a model that lives under a
/// support root (`lib/`, a package's own autoload path, …) instead of
/// `app/models` is ingested by a second loop in `app.rs` (the one
/// `model_roots.rs` covers — "an ActiveRecord class is a model
/// wherever it lives"). That loop's own "classes nested in the
/// model's body are classes of their own" second walk still called
/// the general-purpose `ingest_library_classes` — the exact bug this
/// file's other tests fixed for `app/models`, left unfixed here. An
/// enum alias in a lib-rooted model dropped its nested class with no
/// diagnostic at all.
#[test]
fn a_support_root_model_with_an_enum_alias_still_ingests_its_nested_class() {
    let source = r#"class Widget < ApplicationRecord
  enum :state, { idle: 0, accepted: 1 }
  alias enabled? accepted?

  class Badge
    def self.describe
      "nested"
    end
  end
end
"#;
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("lib/pkg/widget.rb", source),
    ]))
    .expect("must ingest despite the enum alias");

    // The model itself ingested (not dropped as a library class).
    assert!(
        app.models.iter().any(|m| m.name.0.as_str() == "Widget"),
        "Widget must ingest as a model: {:?}",
        app.models.iter().map(|m| m.name.0.as_str()).collect::<Vec<_>>()
    );

    let nested = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Widget::Badge")
        .expect("the nested class must still be registered as a library class");
    assert!(
        nested.methods.iter().any(|m| m.name.as_str() == "describe"),
        "the nested class's own method must survive: {nested:?}"
    );

    assert!(
        app.library_classes.iter().all(|c| c.name.0.as_str() != "Widget"),
        "the model's own class must not be duplicated as a library class: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}
