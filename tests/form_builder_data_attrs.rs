//! A form field's `data:` hash must DASHERIZE its keys.
//!
//! Rails' `tag_options` renders `data: { upload_preview_target: "input" }`
//! as `data-upload-preview-target="input"` — measured against Rails 8.1:
//!
//! ```text
//! tag.input(data: { upload_preview_target: "input" })
//!   → <input data-upload-preview-target="input">
//! ```
//!
//! `data-upload_preview_target` is a DIFFERENT attribute, so a Stimulus
//! controller bound to the dasherized name never fires. The form builder
//! keeps its own `data:` loop (separate from `attr_parts`, which the tag
//! builder uses and which already kebab-cases) for the runtime nil-guard
//! it adds; that copy dropped the dasherizing, so every multi-word
//! Stimulus target on a form input was silently inert. Neither fixture
//! writes a multi-word data key, which is why it went unseen.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_view(view_src: &str) -> String {
    let tree = vec![
        (
            std::path::PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/user.rb"),
            b"class User < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("app/views/users/_form.html.erb"),
            view_src.as_bytes().to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let view = app.views.first().expect("view ingested");
    let lc = lower_view_to_library_class(view, &app);
    format!("{:?}", lc.methods.first().expect("view method").body)
}

#[test]
fn multi_word_data_key_is_dasherized() {
    let body = lower_view(
        "<%= form_with model: user do |f| %>\n<%= f.text_field :name, data: { upload_preview_target: \"input\" } %>\n<% end %>\n",
    );
    assert!(
        body.contains("data-upload-preview-target"),
        "expected the key dasherized:\n{body}"
    );
    assert!(
        !body.contains("data-upload_preview_target"),
        "underscores must not survive — that is a different attribute:\n{body}"
    );
}

#[test]
fn single_word_data_key_is_unchanged() {
    let body = lower_view(
        "<%= form_with model: user do |f| %>\n<%= f.text_field :name, data: { action: \"go\" } %>\n<% end %>\n",
    );
    assert!(body.contains("data-action"), "{body}");
}
