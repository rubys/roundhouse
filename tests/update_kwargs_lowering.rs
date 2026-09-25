//! Kwargs-form update inlining (`lower::apply_update_kwargs_inline`).
//!
//! Shape tests over the rewrite: a model-typed receiver's literal
//! sym-key `update!` inlines to writer assigns + `save!` (plain form
//! saves), a Hash receiver stays put silently (`Hash#update` is
//! `merge!`), an unknown-typed receiver stays put WITH a
//! `lower_residue` warning, and a non-literal argument (the
//! strong-params shape) is not this pass's business.

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::{ingest_library_classes, ingest_model, ingest_schema};
use roundhouse::lower::apply_update_kwargs_inline;
use roundhouse::App;

/// A one-model app (Invitation with a `code` column) plus the given
/// library-class source, analyzed and run through the pass.
fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let schema = ingest_schema(
        br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "invitations", force: :cascade do |t|
    t.string "code"
    t.integer "new_user_id", null: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");
    let model = ingest_model(
        br#"
class Invitation < ApplicationRecord
  belongs_to :new_user, class_name: "User"
end
"#,
        "app/models/invitation.rb",
        &schema,
        &Default::default(),
    )
    .expect("ingest model")
    .expect("model recognized");

    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    app.schema = schema;
    app.models.push(model);
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_update_kwargs_inline(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn model_receiver_bang_form_inlines_to_writer_assigns_and_save_bang() {
    let (out, diags) = lower_and_emit(
        r#"
class Redeemer
  def redeem(code)
    invitation = Invitation.find_by(code: code)
    invitation.update!(code: "used", new_user: nil)
  end
end
"#,
    );
    assert!(!out.contains("update!"), "site should be inlined:\n{out}");
    assert!(out.contains(r#"invitation.code = "used""#), "column assign:\n{out}");
    assert!(out.contains("invitation.new_user = nil"), "assoc assign:\n{out}");
    assert!(out.contains("invitation.save!"), "bang form saves with bang:\n{out}");
    assert!(diags.is_empty(), "matching site should not produce residue: {diags:?}");
}

#[test]
fn plain_form_saves_without_bang() {
    let (out, diags) = lower_and_emit(
        r#"
class Redeemer
  def redeem(code)
    invitation = Invitation.find_by(code: code)
    invitation.update(code: "used")
  end
end
"#,
    );
    assert!(!out.contains("invitation.update("), "{out}");
    assert!(out.contains("invitation.save"), "{out}");
    assert!(!out.contains("save!"), "plain form must not bang:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn hash_receiver_stays_put_silently() {
    let (out, diags) = lower_and_emit(
        r#"
class Merger
  def merge_defaults
    opts = { a: 1 }
    opts.update(b: 2)
  end
end
"#,
    );
    assert!(out.contains("opts.update(b: 2)"), "Hash#update is merge!:\n{out}");
    assert!(
        diags.is_empty(),
        "positively-typed non-model receiver is correct as-is: {diags:?}"
    );
}

#[test]
fn unknown_receiver_stays_put_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Toucher
  def touch(record)
    record.update!(code: "x")
  end
end
"#,
    );
    assert!(out.contains("update!"), "unknown receiver must not rewrite:\n{out}");
    assert_eq!(diags.len(), 1, "expected one residue note: {diags:?}");
    assert!(
        diags[0].message.contains("not typed to a model"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn non_literal_argument_is_not_this_pass() {
    let (out, diags) = lower_and_emit(
        r#"
class Redeemer
  def redeem(code, attrs)
    invitation = Invitation.find_by(code: code)
    invitation.update(attrs)
  end
end
"#,
    );
    assert!(
        out.contains("invitation.update(attrs)"),
        "dynamic-arg update dispatches to the synthesized hash-bag update:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn touch_with_a_column_stamps_it_then_touches() {
    // lobsters' read markers: `@user&.touch(:last_read_newest_story)`.
    // `Base#touch` takes no column (a shared `touch(name)` would
    // index-write through a variable key), so the column write is
    // inlined at the call site and the record's own `touch` finishes.
    let (out, diags) = lower_and_emit(
        r#"
class Marker
  def mark(code)
    invitation = Invitation.find_by(code: code)
    invitation&.touch(:code)
  end
end
"#,
    );
    assert!(!out.contains("touch(:code)"), "site should be inlined:\n{out}");
    assert!(out.contains("__update_rcv.code = ActiveSupport.db_now"), "column stamped:\n{out}");
    assert!(out.contains("__update_rcv.touch"), "then touched:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn bare_touch_is_left_to_the_runtime() {
    let (out, _) = lower_and_emit(
        r#"
class Marker
  def mark(code)
    Invitation.find_by(code: code).touch
  end
end
"#,
    );
    assert!(out.contains(".touch"), "{out}");
    assert!(!out.contains("db_now"), "{out}");
}
