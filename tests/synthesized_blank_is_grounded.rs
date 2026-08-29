//! A `blank?` a LOWERING synthesizes must arrive grounded, because the
//! grounding pass has already run by the time it is built.
//!
//! `lower::apply_blank_lowering` walks the app's bodies once, early,
//! and rewrites `blank?` by the receiver's stamped type. Two passes
//! synthesize a method body much later, inside `model_to_library`, and
//! both used to spell `blank?` themselves:
//!
//! * `lower::secure_token` — `has_secure_token`'s `before_create`.
//! * `model_to_library::markers::rewrite_column_or_assign` — a
//!   `self.<col> ||= v` on a string column.
//!
//! Nothing caught it. The grounding pass reports what it could not
//! ground as `blank_unlowered`, but only for sites it WALKED, and it
//! never walked these — so the emit carried a bare dynamic `blank?`
//! with no error and no warning. On the CRuby overlay that is
//! invisible: ActiveSupport reopens `Object`. On spinel it compiled and
//! then raised `NoMethodError: undefined method 'blank?' for an
//! instance of String` on the first request that ran the callback —
//! campfire's `POST /first_run`, which creates a `Session`.
//!
//! The guard grounds to `ActiveSupport.blank?`, the value-branching
//! runtime predicate, and NOT to the String form `(r || "").strip
//! .empty?` — a string COLUMN does not mean a String VALUE. Rails casts
//! on assignment; this runtime's writer is a bare `@col = value`, so
//! `create!(client_message_id: 999)` leaves an Integer in the attribute
//! when `before_create` runs. `the_column_type_is_not_the_value_type`
//! below is that case, and it was a green campfire test that the String
//! grounding turned red.
//!
//! THE GATE HAS TO READ THE LOWERED OUTPUT. `tests/blank_lowering.rs`
//! calls `apply_blank_lowering` directly and passes on every one of
//! these shapes, because the synthesis it is blind to happens in a pass
//! it does not run. These tests go through `emit_lowered_models`, which
//! is what actually ships.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "sessions", force: :cascade do |t|
    t.string "token", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.string "client_message_id", null: false
  end
end
"#;

fn model_src(name: &str) -> String {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/session.rb",
            r#"class Session < ApplicationRecord
  has_secure_token
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  before_create -> { self.client_message_id ||= Random.uuid }
end
"#,
        ),
    ]))
    .expect("ingest synthesized-blank app");

    let files = ruby::emit_lowered_models(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

/// `has_secure_token` fills the column in `before_create`, guarded by a
/// blankness test — `blank?`, not `nil?`, because this runtime defaults
/// a string slot to `""`. The guard has to be the GROUNDED test.
#[test]
fn has_secure_token_guard_is_grounded() {
    let src = model_src("session.rb");
    assert!(
        src.contains("def before_create"),
        "the macro must synthesize the callback:\n{src}"
    );
    assert!(
        !src.contains("self.token.blank?"),
        "synthesized `blank?` reaches the emitters undispatched:\n{src}"
    );
    assert!(
        src.contains("ActiveSupport.blank?(self.token)"),
        "expected the runtime predicate:\n{src}"
    );
}

/// `self.<col> ||= v` on a string column becomes an `if <col>.blank?`,
/// for the same reason — and through the same hole.
#[test]
fn column_or_assign_guard_is_grounded() {
    let src = model_src("message.rb");
    assert!(
        !src.contains("self.client_message_id.blank?"),
        "synthesized `blank?` reaches the emitters undispatched:\n{src}"
    );
    assert!(
        src.contains("ActiveSupport.blank?(self.client_message_id)"),
        "expected the runtime predicate:\n{src}"
    );
}

/// THE REASON IT IS THE RUNTIME PREDICATE AND NOT `strip.empty?`.
///
/// A `t.string` column does not mean the attribute holds a String when
/// the callback runs. Rails type-casts on assignment; the writer this
/// runtime generates is a bare `@col = value`, so an Integer handed to
/// a string column is still an Integer in `before_create`. campfire's
/// suite does exactly that — `create! client_message_id: 999` —, and
/// the String grounding raised `undefined method 'strip' for an
/// instance of Integer` on a test that had been green.
///
/// `ActiveSupport.blank?` branches on the value, so it answers for an
/// Integer as ActiveSupport's `Object#blank?` did, and it carries the
/// same whitespace rule for the String case (`" "` is blank), which is
/// what keeps a synthesized site and a source-written one in agreement.
#[test]
fn the_column_type_is_not_the_value_type() {
    for (file, col) in [("session.rb", "token"), ("message.rb", "client_message_id")] {
        let src = model_src(file);
        let guard = src
            .lines()
            .find(|l| l.contains("ActiveSupport.blank?"))
            .unwrap_or_else(|| panic!("no blankness guard in {file}:\n{src}"));
        assert!(
            guard.contains(&format!("ActiveSupport.blank?(self.{col})")),
            "{file}: guard must branch on the VALUE, not assume the column type:\n  {guard}"
        );
        assert!(
            !guard.contains("strip"),
            "{file}: `strip` assumes a String and raises on the Integer a \
             string column accepts here:\n  {guard}"
        );
    }
}
