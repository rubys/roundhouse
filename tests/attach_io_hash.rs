//! Active Storage `attach` Hash / `io:` / one-argument attachable
//! lowering (`lower::attached`). Literal `io:` hashes ground to the
//! three-string `attach`; a yielded bag becomes `from_attachable`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn schema() -> &'static str {
    r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
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
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end
end
"#
}

fn emitted_model(model: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (PathBuf::from("db/schema.rb"), schema().as_bytes().to_vec()),
        (PathBuf::from("app/models/doc.rb"), model.as_bytes().to_vec()),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("doc.rb"))
        .map(|f| f.content)
        .expect("doc.rb emitted")
}

#[test]
fn a_literal_io_hash_grounds_to_three_strings() {
    let src = emitted_model(
        r#"class Doc < ApplicationRecord
  has_one_attached :file
  def put_literal
    file.attach(io: StringIO.new("bytes"), filename: "a.png", content_type: "image/png")
  end
end
"#,
    );
    let body = method_body(&src, "put_literal");
    let io_at = body.find("_attach_io").expect("_attach_io bind");
    let fn_at = body.find("_attach_filename").expect("_attach_filename bind");
    assert!(
        io_at < fn_at && body.contains("\"bytes\"") && body.contains("attach(_attach_io"),
        "literal io hash must bind io before filename, then attach bytes:\n{body}"
    );
    assert!(
        body.contains("\"image/png\""),
        "supplied content_type must be kept:\n{body}"
    );
    assert!(
        !body.contains("from_attachable"),
        "literal three-key io hash must not take the bag path:\n{body}"
    );
}

#[test]
fn io_and_filename_without_content_type_uses_the_filename() {
    let src = emitted_model(
        r#"class Doc < ApplicationRecord
  has_one_attached :file
  def put_named
    file.attach(io: StringIO.new("bytes"), filename: "a.png")
  end
end
"#,
    );
    let body = method_body(&src, "put_named");
    assert!(
        body.contains("content_type_for_filename"),
        "missing content_type must be the filename's type:\n{body}"
    );
    assert!(
        body.contains("_attach_io") && body.contains("attach(_attach_io"),
        "io must bind then attach the bound bytes:\n{body}"
    );
}

#[test]
fn nil_content_type_is_treated_as_absent() {
    let src = emitted_model(
        r#"class Doc < ApplicationRecord
  has_one_attached :file
  def put_nil_type
    file.attach(io: StringIO.new("bytes"), filename: "a.png", content_type: nil)
  end
end
"#,
    );
    let body = method_body(&src, "put_nil_type");
    assert!(
        body.contains("content_type_for_filename") && body.contains("_attach_content_type.nil?"),
        "nil content_type must fall back to the filename:\n{body}"
    );
}

#[test]
fn a_hash_bag_becomes_attach_attachable() {
    let src = emitted_model(
        r#"class Doc < ApplicationRecord
  has_one_attached :file
  def put_bag(attachment)
    file.attach(attachment)
  end
end
"#,
    );
    let body = method_body(&src, "put_bag");
    assert!(
        body.contains("_attach_recv")
            && body.contains("from_attachable(attachment)")
            && body.contains("attach_blob"),
        "bag must bind recv first then from_attachable:\n{body}"
    );
}

#[test]
fn positional_three_strings_stay_attach() {
    let src = emitted_model(
        r#"class Doc < ApplicationRecord
  has_one_attached :file
  def put_positional
    file.attach("bytes", "a.png", "image/png")
  end
end
"#,
    );
    let body = method_body(&src, "put_positional");
    assert!(
        body.contains("attach(\"bytes\", \"a.png\", \"image/png\")"),
        "positional attach must stay three strings:\n{body}"
    );
    assert!(
        !body.contains("from_attachable"),
        "positional attach is not a bag:\n{body}"
    );
}

fn method_body<'a>(src: &'a str, name: &str) -> &'a str {
    let needle = format!("def {name}");
    let at = src.find(&needle).unwrap_or_else(|| panic!("missing {name} in\n{src}"));
    let rest = &src[at..];
    rest.split("\n  def ").next().unwrap_or(rest)
}
