//! Per-model `cache_key` / `cache_key_with_version`
//! (`model_to_library::markers::push_cache_key_methods`).
//!
//! The identity a fragment cache keys on. What has to hold is narrow:
//! the key is distinct per row, and it CHANGES when the row's
//! `updated_at` does — which, with `belongs_to … touch:` cascading, is
//! how a boost invalidates the message fragment that renders it.
//!
//! The key is never compared against one Rails wrote (the store is
//! ours), so the two places this deliberately diverges from Rails are
//! free: the prefix is the TABLE name, and the version is the stored
//! `<col>_raw` text rather than `updated_at.utc.to_fs(:usec)`.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::{ingest_model, ingest_schema};
use roundhouse::App;

const SCHEMA: &[u8] = br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "rooms", force: :cascade do |t|
    t.string   "name"
    t.string   "type"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "messages", force: :cascade do |t|
    t.integer  "room_id"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "searches", force: :cascade do |t|
    t.string "query"
  end
end
"#;

fn emit() -> String {
    let schema = ingest_schema(SCHEMA, "db/schema.rb").expect("ingest schema");
    let mut app = App::new();
    for (src, path) in [
        ("class Room < ApplicationRecord\n  has_many :messages\nend\n", "app/models/room.rb"),
        (
            "class Message < ApplicationRecord\n  belongs_to :room, touch: true\nend\n",
            "app/models/message.rb",
        ),
        // No timestamps at all — Rails' `cache_version` is nil here.
        ("class Search < ApplicationRecord\nend\n", "app/models/search.rb"),
        // STI on `rooms`: same table, same row identity.
        ("class Rooms::Open < Room\nend\n", "app/models/rooms/open.rb"),
    ] {
        let model = ingest_model(src.as_bytes(), path, &schema, &Default::default())
            .expect("ingest model")
            .expect("model recognized");
        app.models.push(model);
    }
    app.schema = schema;
    Analyzer::new(&app).analyze(&mut app);
    emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `def <name>` inside the first `class <marker>` block.
fn method<'a>(out: &'a str, marker: &str, name: &str) -> &'a str {
    let at = out.find(marker).unwrap_or_else(|| panic!("no {marker}:\n{out}"));
    let rest = &out[at..];
    let start = rest
        .find(&format!("def {name}\n"))
        .unwrap_or_else(|| panic!("{marker} has no `def {name}`:\n{out}"));
    let body = &rest[start..];
    let end = body.find("\n  end\n").map(|i| i + 7).unwrap_or(body.len());
    &body[..end]
}

#[test]
fn the_key_is_table_slash_id_and_the_version_is_the_stored_timestamp() {
    let out = emit();
    let key = method(&out, "class Message", "cache_key");
    assert!(key.contains(r#""messages/#{@id}""#), "table/id:\n{key}");

    let versioned = method(&out, "class Message", "cache_key_with_version");
    assert!(
        versioned.contains("@updated_at_raw"),
        "version reads the STORED text, not a re-formatted Time:\n{versioned}",
    );
    assert!(
        versioned.contains("cache_key"),
        "the versioned key builds on the plain one:\n{versioned}",
    );
}

#[test]
fn an_unsaved_record_gets_rails_own_new_key() {
    // Not a useful entry, but a STABLE string. The alternative is every
    // unsaved record of a class sharing `"messages/0"` — a silent
    // wrong-bytes collision.
    //
    // Through `self.persisted?`, NOT `new_record?`. Only `persisted?`
    // is synthesized per model on every target (rust emits it as
    // `self.id != 0`, go as a field read); `new_record?` lives on the
    // runtime Base, which a rust struct and a go struct do not inherit
    // — spelling it that way broke four CI jobs with `cannot find
    // function new_record_pred` and `undefined: NewRecord`.
    let out = emit();
    let key = method(&out, "class Message", "cache_key");
    assert!(key.contains("self.persisted?"), "guarded on persisted?:\n{key}");
    assert!(!key.contains("new_record?"), "never new_record?:\n{key}");
    assert!(key.contains(r#""messages/new""#), "Rails' own unsaved key:\n{key}");
}

#[test]
fn a_model_with_no_updated_at_has_no_version_suffix() {
    // Rails: `cache_version` is nil and `cache_key_with_version` is
    // just the key. Such a row can only be invalidated by its key
    // changing — Rails' exposure too, not a new one.
    let out = emit();
    let versioned = method(&out, "class Search", "cache_key_with_version");
    assert!(!versioned.contains("_raw"), "nothing to version by:\n{versioned}");
    assert!(versioned.contains("cache_key"), "still answers the plain key:\n{versioned}");
}

#[test]
fn an_sti_subclass_inherits_the_base_key_rather_than_minting_its_own() {
    // Rails would say `rooms/opens` for the subclass and `rooms` for
    // the base — two entries for ONE row. An STI subclass's `table` is
    // the class-derived `opens`, in no schema, so it defines nothing
    // and inherits `Room#cache_key`: one row, one key, whichever read
    // reached it (this emit hydrates an association's records as the
    // BASE class, so a class-derived prefix would make the key depend
    // on the read).
    let out = emit();
    let sub = out
        .split("class Open < Room")
        .nth(1)
        .expect("Rooms::Open emitted");
    let sub = &sub[..sub.find("\nend\n").unwrap_or(sub.len())];
    assert!(
        !sub.contains("def cache_key"),
        "the subclass must define no key of its own:\n{sub}",
    );
    assert!(
        method(&out, "class Room ", "cache_key").contains(r#""rooms/#{@id}""#),
        "the base owns it:\n{out}",
    );
}
