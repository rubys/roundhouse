//! `with_attached_<attr>` / `with_rich_text_<attr>` are callable ON A
//! RELATION, not only on the model class.
//!
//! These scopes are SYNTHESIZED beside the attachment macro at emit
//! time; they never pass through `build_scope_registry`, which reads the
//! app's own `scope` declarations. So `relation_scopes.rb` — the
//! Relation reopen that lets a scope be chained mid-chain on a relation
//! VALUE — had no delegate for them, and campfire's
//! `find_autocompletable_users.with_attached_avatar.ordered` was a
//! NoMethodError on a method that plainly exists on `User`.
//!
//! The delegate is `self`, with no `__scope_` dispatch behind it: these
//! scopes ARE identity here (Rails' `includes(...)` is a query-plan
//! hint, and the per-record readers this compiler synthesizes have
//! nothing for it to attach to). So there is no arity to detect and no
//! model to choose.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const MODEL: &str = r#"class User < ApplicationRecord
  has_one_attached :avatar
  scope :ordered, -> { order(:name) }
end
"#;

fn delegates() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", MODEL),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .chain(ruby::emit_lowered_models(&app).iter())
        .find(|f| f.path.to_string_lossy().ends_with("relation_scopes.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            roundhouse::emit::ruby::emit_spinel(&app)
                .iter()
                .find(|f| f.path.to_string_lossy().ends_with("relation_scopes.rb"))
                .map(|f| f.content.clone())
                .expect("relation_scopes.rb emitted")
        })
}

#[test]
fn a_synthesized_preload_scope_gets_an_identity_delegate() {
    let src = delegates();
    assert!(
        src.contains("def with_attached_avatar"),
        "the preload scope is delegated:\n{src}"
    );
    let at = src.find("def with_attached_avatar").unwrap();
    assert!(
        src[at..].starts_with("def with_attached_avatar\n      self\n"),
        "the delegate is identity, with no __scope_ dispatch:\n{}",
        &src[at..(at + 80).min(src.len())]
    );
}

/// The declared scope beside it keeps its real delegate — the identity
/// arm must not shadow a scope that has a body.
#[test]
fn a_declared_scope_keeps_its_own_delegate() {
    let src = delegates();
    assert!(
        src.contains("def ordered"),
        "the declared scope is still delegated:\n{src}"
    );
    let at = src.find("def ordered").unwrap();
    assert!(
        !src[at..].starts_with("def ordered\n      self\n"),
        "a declared scope must not be replaced by identity:\n{}",
        &src[at..(at + 120).min(src.len())]
    );
}
