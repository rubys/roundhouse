//! Named plain-text association — `has_markdown :name` and the
//! `ActionText::Markdown` record (`lower::plain_text_attr`).
//!
//! Pins the association+storage expansion as a general Rails surface:
//! scoped `markdown_<name>` has_one, reader/predicate/writer through
//! `.content`, ordinary autosave, writable virtual attr. Writebook's
//! `Page#body` is extra fixture coverage only.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;
use roundhouse::App;

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :articles do |t|
    t.string :title
    t.timestamps
  end

  create_table :action_text_markdowns do |t|
    t.text :content
    t.string :name, null: false
    t.bigint :record_id, null: false
    t.string :record_type, null: false
    t.timestamps
  end
end
"#;

const ARTICLE: &str = r#"class Article < ApplicationRecord
  has_markdown :body
end
"#;

fn ingest(schema: &str, model: &str) -> App {
    let files: Vec<(&str, &str)> = vec![("db/schema.rb", schema), ("app/models/article.rb", model)];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn app() -> App {
    ingest(SCHEMA, ARTICLE)
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
fn declaring_has_markdown_synthesizes_the_record_model() {
    let app = app();
    let md = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "ActionText::Markdown")
        .expect("ActionText::Markdown synthesized into app.models");
    assert_eq!(md.table.0.as_str(), "action_text_markdowns");
}

#[test]
fn an_app_without_the_declaration_gets_no_record_model() {
    let app = ingest(SCHEMA, "class Article < ApplicationRecord\nend\n");
    assert!(
        !app.models
            .iter()
            .any(|m| m.name.0.as_str() == "ActionText::Markdown"),
        "no has_markdown declaration must mean no synthesized record"
    );
}

#[test]
fn a_schema_without_the_table_gets_no_record_model() {
    let schema = "ActiveRecord::Schema.define(version: 1) do\n  \
                  create_table :articles do |t|\n    t.string :title\n  end\nend\n";
    let app = ingest(schema, ARTICLE);
    assert!(
        !app.models
            .iter()
            .any(|m| m.name.0.as_str() == "ActionText::Markdown"),
        "no action_text_markdowns table must mean no synthesized record"
    );
}

#[test]
fn the_declaring_model_gets_the_macro_expansion() {
    let app = app();
    let lc = lower(&app, "Article");
    for name in ["markdown_body", "build_markdown_body", "body", "body?", "body="] {
        assert!(
            instance_method(&lc, name).is_some(),
            "has_markdown :body must synthesize `{name}`"
        );
    }
    let ret = match &instance_method(&lc, "body").unwrap().signature {
        Some(roundhouse::ty::Ty::Fn { ret, .. }) => (**ret).clone(),
        other => panic!("body reader has no Fn signature: {other:?}"),
    };
    assert!(
        matches!(&ret, roundhouse::ty::Ty::Class { id, .. }
            if id.0.as_str() == "ActionText::Markdown"),
        "Article#body must be non-nullable ActionText::Markdown, got {ret:?}"
    );
}

#[test]
fn the_owner_saves_its_plain_text_after_save() {
    let app = app();
    let lc = lower(&app, "Article");
    assert!(
        instance_method(&lc, "_save_markdown_body").is_some(),
        "the autosave half must be synthesized"
    );
    let after_save = instance_method(&lc, "after_save")
        .expect("after_save synthesized to carry the plain-text save");
    let body = format!("{:?}", after_save.body);
    assert!(
        body.contains("_save_markdown_body"),
        "after_save must call the plain-text save: {body}"
    );
}

#[test]
fn a_plain_text_attribute_is_assignable() {
    let app = app();
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    let table = app
        .schema
        .tables
        .get(&roundhouse::ident::Symbol::from("articles"))
        .unwrap();
    let writable = roundhouse::lower::model_to_library::writable_field_set(model, table);
    assert!(
        writable.contains(&roundhouse::ident::Symbol::from("body")),
        "has_markdown :body must be writable for permit/create"
    );
}

#[test]
fn preload_scope_names_match_rails() {
    let app = app();
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    let names: Vec<String> = roundhouse::lower::plain_text_attr::preload_scope_names(model)
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["with_markdown_body", "with_markdown_body_and_embeds"]
    );
}

#[test]
fn option_carrying_declaration_stays_unclaimed() {
    let app = ingest(
        SCHEMA,
        "class Article < ApplicationRecord\n  has_markdown :body, strict_loading: true\nend\n",
    );
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert!(
        roundhouse::lower::plain_text_attr::plain_text_attrs(model).is_empty(),
        "strict_loading: form must not be claimed"
    );
}

#[test]
fn load_hook_installer_leaves_bare_call_for_the_lowerer() {
    // Writebook installs HasMarkdown via on_load; the class_eval path
    // must not swallow the bare call into an ingest gap.
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "lib/rails_ext/action_text_has_markdown.rb",
            r#"module ActionText
  module HasMarkdown
    extend ActiveSupport::Concern
    class_methods do
      def has_markdown(name, strict_loading: strict_loading_by_default)
        class_eval <<-CODE, __FILE__, __LINE__ + 1
          def #{name}
            markdown_#{name} || build_markdown_#{name}
          end
          def #{name}?
            markdown_#{name}.present?
          end
          def #{name}=(content)
            self.#{name}.content = content
          end
        CODE
        has_one :"markdown_#{name}", -> { where(name: name) },
          class_name: "ActionText::Markdown", as: :record, inverse_of: :record,
          autosave: true, dependent: :destroy, strict_loading: strict_loading
        scope :"with_markdown_#{name}", -> { includes("markdown_#{name}") }
        scope :"with_markdown_#{name}_and_embeds",
          -> { includes("markdown_#{name}": { embeds_attachments: :blob }) }
      end
    end
  end
end
ActiveSupport.on_load :active_record do
  include ActionText::HasMarkdown
end
"#,
        ),
        ("app/models/article.rb", ARTICLE),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest with load-hook installer");
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(
        roundhouse::lower::plain_text_attr::plain_text_attrs(model)
            .iter()
            .map(|(_, a)| a.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["body"]
    );
    let lc = lower(&app, "Article");
    assert!(instance_method(&lc, "markdown_body").is_some());
    assert!(instance_method(&lc, "body=").is_some());
}
