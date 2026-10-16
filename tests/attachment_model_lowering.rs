//! `ActiveStorage::Attachment` synthesis and `has_many_attached`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
    t.bigint "author_id"
  end
  create_table "authors", force: :cascade do |t|
    t.string "email"
  end
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.string "slug"
    t.datetime "created_at", null: false
  end
  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
  end
end
"#;

#[test]
fn attachment_model_is_synthesized_when_table_present() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/doc.rb", "class Doc < ApplicationRecord\nend\n"),
    ]))
    .expect("ingest");
    assert!(
        app.models
            .iter()
            .any(|m| m.name.0.as_str() == "ActiveStorage::Attachment"),
        "Attachment must be synthesized when its table exists"
    );
}

#[test]
fn has_many_attached_reader_is_attached_many() {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/doc.rb",
            "class Doc < ApplicationRecord\n  has_many_attached :uploads\nend\n",
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("doc.rb"))
        .expect("doc.rb")
        .content;
    assert!(
        src.contains(r#"ActiveStorage::AttachedMany.new("Doc", @id, "uploads")"#),
        "has_many_attached must synthesize an AttachedMany reader:\n{src}"
    );
    assert!(
        src.contains("with_attached_uploads"),
        "preload scope must exist:\n{src}"
    );
}

#[test]
fn on_load_active_storage_attachment_include_reaches_model() {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/doc.rb",
            "class Doc < ApplicationRecord\n  belongs_to :author\nend\n",
        ),
        (
            "app/models/author.rb",
            "class Author < ApplicationRecord\nend\n",
        ),
        (
            "app/models/concerns/doc/uploads.rb",
            r#"module Doc::Uploads
  extend ActiveSupport::Concern
  included do
    has_many_attached :uploads
    delegate :email, to: :author
  end
end
"#,
        ),
        (
            "lib/rails_ext/doc_uploads.rb",
            r#"ActiveSupport.on_load :active_storage_attachment do
  class Doc
    include Doc::Uploads
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("doc.rb"))
        .expect("doc.rb")
        .content;
    assert!(
        src.contains("AttachedMany.new"),
        "on_load include must splice has_many_attached onto Doc:\n{src}"
    );
    assert!(
        src.contains("def email\n    self.author.email\n  end"),
        "a delegate added by the late on_load concern splice must be expanded:\n{src}"
    );
}

#[test]
fn a_private_delegate_in_an_included_block_is_rejected_at_ingest() {
    let error = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/doc.rb",
            "class Doc < ApplicationRecord\n  belongs_to :author\n  include Doc::PrivateEmail\nend\n",
        ),
        (
            "app/models/concerns/doc/private_email.rb",
            r#"module Doc::PrivateEmail
  extend ActiveSupport::Concern
  included do
    private
    delegate :email, to: :author
  end
end
"#,
        ),
    ]))
    .expect_err("private concern delegates are not modeled");
    assert!(
        error
            .to_string()
            .contains("conditional or dynamic visibility declarations are not modeled"),
        "unexpected ingest error: {error}"
    );
}
