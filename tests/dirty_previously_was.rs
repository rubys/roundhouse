//! `<col>_previously_was` and the hydration baseline it reads through.
//!
//! The predicates (`<col>_previously_changed?` / `saved_change_to_<col>?`)
//! answer WHETHER the last save changed a column; this is the value
//! half — what it held before. campfire decides whether a room
//! appearing in a sidebar is a new grant or a visibility change with
//! `@membership.involvement_previously_was.inquiry.invisible?`.
//!
//! The baseline is the other half. `__track_saved_changes` diffs against
//! the previous save's snapshot, and a record HYDRATED from the DB had
//! none — so its first update reported `[nil, value]` for every column
//! and this reader answered nil for all of them. connection.rb's own
//! comment deferred that ("baseline-at-hydration is future work"); the
//! hydration factories now call `__note_hydrated`, whose Base no-op is
//! what the strict lanes compile.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"memberships\", force: :cascade do |t|\n    t.string \"involvement\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/membership.rb"),
            b"class Membership < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("membership.rb"))
        .expect("no membership.rb emitted")
        .content
        .clone()
}

#[test]
fn the_value_half_reads_the_prev_slot_of_the_diff_pair() {
    let src = emitted();
    assert!(src.contains("def involvement_previously_was"), "{src}");
    assert!(src.contains("saved_changes[:involvement]"), "{src}");
    // `[prev, value]` — the previous value is slot 0.
    assert!(src.contains("[0]"), "{src}");
}

/// Nil when the column did not change, which is Rails: the diff has no
/// entry and `attribute_previously_was` reads through it rather than
/// falling back to the current value.
#[test]
fn an_unchanged_column_answers_nil_rather_than_its_current_value() {
    let src = emitted();
    let body = src
        .split("def involvement_previously_was")
        .nth(1)
        .expect("no reader")
        .split("end")
        .next()
        .unwrap_or("");
    assert!(body.contains("nil?"), "{src}");
    assert!(body.contains("nil"), "{src}");
}

/// Both hydration factories take the baseline — a record read through
/// `from_stmt` is as much "already saved" as one read through `from_row`.
#[test]
fn both_hydration_factories_note_the_baseline() {
    let src = emitted();
    let from_row = src.split("def self.from_row").nth(1).expect("no from_row");
    let (from_row_body, rest) = from_row.split_once("def self.from_stmt").expect("no from_stmt");
    assert!(from_row_body.contains("__note_hydrated"), "from_row:\n{src}");
    assert!(rest.contains("__note_hydrated"), "from_stmt:\n{src}");
}

/// `id` is answered by the save path's own flag, not the snapshot diff —
/// the predicates skip it and so does this.
#[test]
fn id_is_not_given_a_previously_was_reader() {
    let src = emitted();
    assert!(!src.contains("def id_previously_was"), "{src}");
}

/// `x.inquiry.<label>?` written at the CALL SITE, where the receiver
/// types as neither String nor a known inquirer-returning method.
///
/// The `Ty::Str` gate cannot see this one — campfire's receiver is the
/// Dirty reader above, whose value comes out of an untyped
/// saved-changes diff — and the `inquirers` set does not cover it
/// either, since that set is for a METHOD whose body ends in
/// `.inquiry`. An explicit `.inquiry` in the source is the strongest
/// evidence of the three and was the one being thrown away: the
/// bottom-up walk folded it off before the predicate was examined,
/// leaving `.invisible?` to raise on a plain String.
#[test]
fn an_inline_inquiry_pair_folds_even_when_the_receiver_is_untyped() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"memberships\", force: :cascade do |t|\n    t.string \"involvement\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/membership.rb"),
            b"class Membership < ApplicationRecord\n  def hidden_before?\n    involvement_previously_was.inquiry.invisible?\n  end\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("membership.rb"))
        .expect("no membership.rb emitted")
        .content
        .clone();
    assert!(
        src.contains("involvement_previously_was == \"invisible\""),
        "{src}",
    );
    assert!(!src.contains(".invisible?"), "no predicate may survive:\n{src}");
    assert!(!src.contains(".inquiry"), "{src}");
}
