//! ActionText::Markdown model recognition (storage only).
//!
//! Writebook ships `lib/rails_ext/action_text_markdown.rb` as
//! `module ActionText; class Markdown < Record`. Without lexical
//! superclass resolution + the framework `action_text_` table prefix,
//! that file lands as a library class and emits `class Markdown < Record`
//! → `NameError` at boot. This suite pins the storage-side fix:
//! model in `app.models`, table `action_text_markdowns`, attr `content`.
//!
//! Does **not** claim `has_markdown`, Page `#body`, renderer, or uploads.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content"
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#;

const MARKDOWN: &str = r#"module ActionText
  class Markdown < Record
    mattr_accessor :renderer
    belongs_to :record, polymorphic: true

    def to_html
      renderer
    end
  end
end
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
"#;

fn ingest(files: &[(&str, &str)]) -> App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn writebook_shaped() -> App {
    ingest(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("lib/rails_ext/action_text_markdown.rb", MARKDOWN),
    ])
}

#[test]
fn markdown_under_action_text_ingests_as_model() {
    let app = writebook_shaped();
    let md = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "ActionText::Markdown")
        .expect("ActionText::Markdown must be a model, not a library class");
    assert!(
        !app.library_classes
            .iter()
            .any(|c| c.name.0.as_str() == "ActionText::Markdown"),
        "must not also remain a library class"
    );
    assert_eq!(md.table.0.as_str(), "action_text_markdowns");
    assert_eq!(
        md.parent.as_ref().map(|p| p.0.as_str()),
        Some("ApplicationRecord"),
        "gem ActionText::Record is classified, then parented ApplicationRecord for emit"
    );
    assert!(
        md.attributes
            .fields
            .contains_key(&roundhouse::ident::Symbol::from("content")),
        "content column from schema; got {:?}",
        md.attributes.fields.keys().collect::<Vec<_>>()
    );
    assert!(
        md.body.iter().any(|item| matches!(
            item,
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "renderer"
                    && method.receiver == roundhouse::dialect::MethodReceiver::Class
        )),
        "mattr_accessor :renderer must synthesize a class reader on the model"
    );
    assert!(
        md.body.iter().any(|item| matches!(
            item,
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "renderer"
                    && method.receiver == roundhouse::dialect::MethodReceiver::Instance
        )),
        "mattr_accessor :renderer must also synthesize an instance reader for to_html"
    );
}

#[test]
fn optioned_mattr_on_a_model_does_not_fail_ingest() {
    // Writebook's ActionText::Markdown uses `mattr_accessor :renderer, default:`.
    // Expanding without the initializer would drop the default; erroring
    // turns existing unresolved sends into ingest-gap Infos. Leave unknown.
    let app = ingest(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        (
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    mattr_accessor :renderer, default: Object.new
  end
end
"#,
        ),
    ]);
    let md = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "ActionText::Markdown")
        .expect("still a model");
    assert!(
        !md.body.iter().any(|item| matches!(
            item,
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "renderer"
        )),
        "optioned mattr must not synthesize a reader that drops default:"
    );
}

#[test]
fn bare_record_outside_action_text_stays_library() {
    let app = ingest(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("lib/other.rb", "class Markdown < Record\nend\n"),
    ]);
    assert!(
        app.models.iter().all(|m| m.name.0.as_str() != "Markdown"),
        "top-level Record must not become a model"
    );
    assert!(
        app.library_classes
            .iter()
            .any(|c| c.name.0.as_str() == "Markdown"),
        "top-level Markdown < Record stays a library class"
    );
}
