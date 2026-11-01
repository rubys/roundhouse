//! Part of #1707: the Active Storage tables in `db/schema.rb` synthesize an
//! `ActiveStorage::Attachment` model, and several lowerings built a
//! constant for it from the class id in ONE path segment
//! (`Const { path: ["ActiveStorage::Attachment"] }`), so Python, Go and
//! Elixir wrote the Ruby spelling into code: `ActiveStorage::Attachment.
//! from_stmt(stmt)`, `ActiveStorage::Attachment_from_stmt(stmt)`. A
//! constant's path now has one segment per name, as everywhere else.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const TABLES: &str = r#"  create_table "active_storage_blobs", force: :cascade do |t|
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

"#;

fn app() -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let schema = format!("{}{TABLES}{}", &schema[..at], &schema[at..]);
    emit_and_run::real_blog().write("db/schema.rb", &schema)
}

/// Every code line of the emitted tree with the Ruby spelling: a comment,
/// or the quoted `"ActiveStorage::Attachment"` an error message
/// interpolates, is the Ruby name on purpose.
fn ruby_spellings(target: BuildTarget, exts: &[&str]) -> Vec<String> {
    let (out, errors) = app().emit(target);
    assert!(errors.is_empty(), "{target:?}: {errors:?}");
    let mut hits = Vec::new();
    let mut stack = vec![out.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if exts.iter().any(|e| path.extension().is_some_and(|x| x == *e)) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for line in text.lines() {
                    let code = line.trim();
                    if code.starts_with("//") || code.starts_with('#') {
                        continue;
                    }
                    let unquoted = code.replace("\"ActiveStorage::Attachment\"", "");
                    if unquoted.contains("ActiveStorage::Attachment") {
                        hits.push(format!("{}: {}", path.file_name().unwrap().to_string_lossy(), line.trim()));
                    }
                }
            }
        }
    }
    hits
}

#[test]
fn python_spells_the_synthesized_attachment_model_as_a_python_name() {
    let hits = ruby_spellings(BuildTarget::Python, &["py"]);
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn go_spells_the_synthesized_attachment_model_as_a_go_name() {
    let hits = ruby_spellings(BuildTarget::Go, &["go"]);
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn elixir_spells_the_synthesized_attachment_model_as_an_elixir_name() {
    let hits = ruby_spellings(BuildTarget::Elixir, &["ex", "exs"]);
    assert!(hits.is_empty(), "{hits:#?}");
}
