//! Action Text — `has_rich_text :body` and the `ActionText::RichText`
//! record it hangs off (`lower::rich_text`).
//!
//! Three things this pins, because each was a wall on campfire's walk:
//!
//! * The record class is SYNTHESIZED into `app.models`. Nothing in the
//!   app declares it, and if it does not appear before the model loop,
//!   half the pipeline never sees it.
//! * `RichText#body` reads back as an `ActionText::Content`, not the
//!   String the column holds. This is `serialize :body, coder:` and it
//!   is what makes `message.body.body.to_plain_text` resolve.
//! * `has_rich_text :body` makes `body` ASSIGNABLE on the declaring
//!   model even though it is not a column — without which the permit
//!   filter drops it and a posted message arrives with no content.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;
use roundhouse::App;

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :messages do |t|
    t.integer :room_id, null: false
    t.timestamps
  end

  create_table :action_text_rich_texts do |t|
    t.text :body
    t.string :name, null: false
    t.bigint :record_id, null: false
    t.string :record_type, null: false
    t.timestamps
  end
end
"#;

const MESSAGE: &str = r#"class Message < ApplicationRecord
  has_rich_text :body

  def plain
    body.to_plain_text
  end
end
"#;

fn ingest(schema: &str, model: &str) -> App {
    let files: Vec<(&str, &str)> = vec![("db/schema.rb", schema), ("app/models/message.rb", model)];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn app() -> App {
    ingest(SCHEMA, MESSAGE)
}

fn lower(app: &App, class: &str) -> roundhouse::dialect::LibraryClass {
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == class)
        .unwrap_or_else(|| panic!("{class} model"));
    lower_model_to_library_class(model, &app.schema)
}

fn instance_method<'a>(
    lc: &'a roundhouse::dialect::LibraryClass,
    name: &str,
) -> Option<&'a roundhouse::dialect::MethodDef> {
    lc.methods
        .iter()
        .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
}

#[test]
fn declaring_has_rich_text_synthesizes_the_record_model() {
    let app = app();
    let rich_text = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "ActionText::RichText")
        .expect("ActionText::RichText synthesized into app.models");
    assert_eq!(rich_text.table.0.as_str(), "action_text_rich_texts");
}

#[test]
fn an_app_without_rich_text_gets_no_record_model() {
    // The pass must be inert for every app that does not use Action
    // Text — an extra model means an extra emitted file and an extra
    // table in every target's schema.
    let app = ingest(SCHEMA, "class Message < ApplicationRecord\nend\n");
    assert!(
        !app.models.iter().any(|m| m.name.0.as_str() == "ActionText::RichText"),
        "no has_rich_text declaration must mean no synthesized record"
    );
}

#[test]
fn a_schema_without_the_table_gets_no_record_model() {
    // `has_rich_text` with no Action Text migration installed: there is
    // nothing to lower against, and synthesizing a model over a missing
    // table would emit SQL against it.
    let schema = "ActiveRecord::Schema.define(version: 1) do\n  \
                  create_table :messages do |t|\n    t.integer :room_id\n  end\nend\n";
    let app = ingest(schema, MESSAGE);
    assert!(
        !app.models.iter().any(|m| m.name.0.as_str() == "ActionText::RichText"),
        "no action_text_rich_texts table must mean no synthesized record"
    );
}

#[test]
fn rich_text_body_reads_back_as_content() {
    let app = app();
    let lc = lower(&app, "ActionText::RichText");
    let reader = instance_method(&lc, "body").expect("body reader");
    let ret = match &reader.signature {
        Some(roundhouse::ty::Ty::Fn { ret, .. }) => (**ret).clone(),
        other => panic!("body reader has no Fn signature: {other:?}"),
    };
    assert!(
        matches!(&ret, roundhouse::ty::Ty::Class { id, .. }
            if id.0.as_str() == "ActionText::Content"),
        "body must read back as ActionText::Content, got {ret:?}"
    );
    // The WRITER still declares the String field — the column keeps its
    // storage, only the read shape changes.
    let writer = instance_method(&lc, "body=").expect("body writer");
    assert_eq!(writer.kind, roundhouse::dialect::AccessorKind::AttributeWriter);
}

#[test]
fn the_record_delegates_the_content_predicates() {
    let app = app();
    let lc = lower(&app, "ActionText::RichText");
    for name in ["to_s", "to_html", "to_plain_text", "to_trix_html", "blank?", "present?", "empty?"]
    {
        assert!(
            instance_method(&lc, name).is_some(),
            "ActionText::RichText must answer `{name}`"
        );
    }
}

#[test]
fn the_declaring_model_gets_the_macro_expansion() {
    let app = app();
    let lc = lower(&app, "Message");
    for name in ["rich_text_body", "build_rich_text_body", "body", "body?", "body="] {
        assert!(
            instance_method(&lc, name).is_some(),
            "has_rich_text :body must synthesize `{name}`"
        );
    }
    // `body` is never nil — it builds when there is no row — so the
    // reader is typed as the record rather than as a nullable one.
    let ret = match &instance_method(&lc, "body").unwrap().signature {
        Some(roundhouse::ty::Ty::Fn { ret, .. }) => (**ret).clone(),
        other => panic!("body reader has no Fn signature: {other:?}"),
    };
    assert!(
        matches!(&ret, roundhouse::ty::Ty::Class { id, .. }
            if id.0.as_str() == "ActionText::RichText"),
        "Message#body must be non-nullable ActionText::RichText, got {ret:?}"
    );
}

#[test]
fn the_owner_saves_its_rich_text_after_save() {
    // `autosave: true`. Without the after_save fold, a created message
    // persists and its body silently does not.
    let app = app();
    let lc = lower(&app, "Message");
    assert!(
        instance_method(&lc, "_save_rich_text_body").is_some(),
        "the autosave half must be synthesized"
    );
    let after_save = instance_method(&lc, "after_save")
        .expect("after_save synthesized to carry the rich-text save");
    let body = format!("{:?}", after_save.body);
    assert!(
        body.contains("_save_rich_text_body"),
        "after_save must call the rich-text save: {body}"
    );
}

#[test]
fn a_rich_text_attribute_is_assignable() {
    // The permit filter asks `writable_field_set` whether a permitted
    // name has a writer. `body` is not a column on `messages`, so
    // without the rich-text entry it is dropped from the synthesized
    // update/from_params and a posted body never reaches the model.
    let app = app();
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Message").unwrap();
    let table = app.schema.tables.get(&roundhouse::ident::Symbol::from("messages")).unwrap();
    let writable = roundhouse::lower::model_to_library::writable_field_set(model, table);
    assert!(
        writable.contains(&roundhouse::ident::Symbol::from("body")),
        "has_rich_text :body must make `body` assignable: {writable:?}"
    );
}

#[test]
fn the_preload_scopes_exist() {
    let app = app();
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Message").unwrap();
    let names: Vec<String> = roundhouse::lower::rich_text::preload_scope_names(model)
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    assert_eq!(names, vec!["with_rich_text_body", "with_rich_text_body_and_embeds"]);
}

#[test]
fn an_option_carrying_declaration_is_not_claimed() {
    // `encrypted:` / `store_if_blank:` each change what the expansion
    // must be, and none is implemented — so the declaration stays
    // unclaimed and keeps reporting, rather than being silently
    // expanded as if the option were absent.
    let app = ingest(
        SCHEMA,
        "class Message < ApplicationRecord\n  has_rich_text :body, encrypted: true\nend\n",
    );
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Message").unwrap();
    assert!(
        roundhouse::lower::rich_text::rich_text_attrs(model).is_empty(),
        "an option-carrying has_rich_text must not be claimed"
    );
    assert!(
        !app.models.iter().any(|m| m.name.0.as_str() == "ActionText::RichText"),
        "an unclaimed declaration must not synthesize the record model"
    );
}
