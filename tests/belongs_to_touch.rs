//! `belongs_to … touch: true` (`model_to_library::markers`).
//!
//! Rails registers the parent-touch on four of the child's hooks
//! (`Builder::BelongsTo.add_touch_callbacks`): create, update, destroy
//! and — the one that makes the cascade transitive — `after_touch`.
//! campfire depends on all four being there: a boost touches its
//! Message, and Message's own `belongs_to :room, touch: true` fires off
//! that touch to reach the Room. Drop `after_touch` and the chain stops
//! one level short, which is invisible to every behavioral test because
//! nothing renders `updated_at`.
//!
//! Shape tests over the emitted models: the hooks are ordinary method
//! definitions from here on, so what is pinned is that they exist, that
//! they guard the nilable reader, and that they read it once.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::{ingest_model, ingest_schema};
use roundhouse::App;

/// A Room/Message/Boost app — campfire's own touch chain, minimized.
/// `decl` is spliced into Boost in place of its `belongs_to`.
fn emit(boost_decl: &str) -> String {
    let schema = ingest_schema(
        br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "rooms", force: :cascade do |t|
    t.string   "name"
    t.datetime "last_active_at"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "messages", force: :cascade do |t|
    t.integer  "room_id"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "boosts", force: :cascade do |t|
    t.integer  "message_id"
    t.string   "content"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");

    let mut app = App::new();
    for (src, path) in [
        (
            "class Room < ApplicationRecord\n  has_many :messages\nend\n".to_string(),
            "app/models/room.rb",
        ),
        (
            "class Message < ApplicationRecord\n  belongs_to :room, touch: true\n  \
             has_many :boosts\nend\n"
                .to_string(),
            "app/models/message.rb",
        ),
        (
            format!("class Boost < ApplicationRecord\n  {boost_decl}\nend\n"),
            "app/models/boost.rb",
        ),
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

/// The body of `def <hook>` in the emitted source, hook name included.
fn hook<'a>(out: &'a str, class_marker: &str, name: &str) -> &'a str {
    let class_at = out.find(class_marker).unwrap_or_else(|| panic!("no {class_marker}:\n{out}"));
    let rest = &out[class_at..];
    let at = rest
        .find(&format!("def {name}\n"))
        .unwrap_or_else(|| panic!("{class_marker} has no `def {name}`:\n{out}"));
    let body = &rest[at..];
    let end = body.find("\n  end\n").map(|i| i + 7).unwrap_or(body.len());
    &body[..end]
}

#[test]
fn touch_true_registers_on_all_four_of_rails_hooks() {
    let out = emit("belongs_to :message, touch: true");
    for name in ["after_create", "after_update", "after_destroy", "after_touch"] {
        let body = hook(&out, "class Boost", name);
        assert!(
            body.contains(".touch"),
            "Boost#{name} must touch the parent:\n{body}",
        );
    }
}

#[test]
fn the_reader_is_bound_once_and_nil_guarded() {
    // The belongs_to reader is the row-LOADING one and its signature is
    // `Message | nil` whatever `optional:` says. A bare `message.touch
    // unless message.nil?` would issue the SELECT twice and read as a
    // nilable receiver at the call.
    let out = emit("belongs_to :message, touch: true");
    let body = hook(&out, "class Boost", "after_create");
    assert!(
        body.contains("__touch_message = self.message"),
        "reader bound to a local:\n{body}",
    );
    assert_eq!(
        body.matches("self.message").count(),
        1,
        "the reader runs ONCE — a second read is a second SELECT:\n{body}",
    );
    assert!(body.contains("__touch_message.nil?"), "guarded on the local:\n{body}");
    assert!(body.contains("__touch_message.touch"), "touched through the local:\n{body}");
}

#[test]
fn the_cascade_is_transitive_through_after_touch() {
    // This is the whole point: Boost#after_touch is what lets a boost
    // reach the Room two levels up, via Message#after_touch.
    let out = emit("belongs_to :message, touch: true");
    assert!(
        hook(&out, "class Message", "after_touch").contains("__touch_room.touch"),
        "Message#after_touch must pass the touch on to its Room:\n{out}",
    );
}

#[test]
fn touch_with_a_column_stamps_it_alongside_updated_at() {
    // Rails' `touch: :last_active_at` stamps that column AND
    // `updated_at`. `ActiveSupport.db_now` rather than `Time.now`: it
    // is the clock the no-arg `touch` beneath it stamps `updated_at`
    // from, so both columns carry one instant and `travel_to` moves
    // both (the same reasoning `column_ops` records).
    let out = emit("belongs_to :message, touch: :last_active_at");
    let body = hook(&out, "class Boost", "after_update");
    assert!(
        body.contains("__touch_message.last_active_at = ActiveSupport.db_now"),
        "named column stamped through the writer:\n{body}",
    );
    assert!(body.contains("__touch_message.touch"), "and `updated_at` still stamped:\n{body}");
}

#[test]
fn touch_false_is_an_opt_out_not_a_touch() {
    let out = emit("belongs_to :message, touch: false");
    assert!(
        !out.contains("__touch_message"),
        "`touch: false` is Rails' explicit opt-out:\n{out}",
    );
}

#[test]
fn a_plain_belongs_to_registers_nothing() {
    let out = emit("belongs_to :message");
    assert!(!out.contains("__touch_message"), "no touch without the option:\n{out}");
}
