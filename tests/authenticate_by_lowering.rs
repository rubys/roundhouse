//! `authenticate_by` grounding (`lower::apply_authenticate_by_lowering`).
//!
//! Shape tests over the Rails 7.1 macro-inline: the keyword hash
//! partitions into `find_by` conditions plus the password to verify,
//! the receiver is bound once (so a Relation receiver works without any
//! class-method delegation), the expansion hoists above the statement
//! that consumed the value, and a call this pass can't ground keeps its
//! source shape WITH a `lower_residue` warning.

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::{ingest_library_classes, ingest_model, ingest_schema};
use roundhouse::lower::apply_authenticate_by_lowering;
use roundhouse::App;

/// A two-model app — User with `has_secure_password`, Article without —
/// plus the given library-class source, analyzed and run through the
/// pass.
fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let schema = ingest_schema(
        br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "users", force: :cascade do |t|
    t.string "email_address"
    t.string "password_digest"
    t.integer "status", default: 0
  end

  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");
    let mut app = App::new();
    for (src, path) in [
        (
            r#"
class User < ApplicationRecord
  has_secure_password validations: false

  scope :active, -> { where(status: 0) }
end
"#,
            "app/models/user.rb",
        ),
        ("class Article < ApplicationRecord\nend\n", "app/models/article.rb"),
    ] {
        let model = ingest_model(src.as_bytes(), path, &schema, &Default::default())
            .expect("ingest model")
            .expect("model recognized");
        app.models.push(model);
    }
    app.schema = schema;

    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_authenticate_by_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn class_receiver_partitions_finders_from_the_password() {
    let (out, diags) = lower_and_emit(
        r#"
class Doorman
  def admit(email, secret)
    found = User.authenticate_by(email_address: email, password: secret)
    found
  end
end
"#,
    );
    assert!(!out.contains("authenticate_by"), "site should be inlined:\n{out}");
    assert!(
        out.contains("__auth = User.find_by(email_address: email)"),
        "finder keys become find_by conditions, receiver bound once:\n{out}",
    );
    assert!(
        out.contains("__auth.authenticate(secret)"),
        "the password key becomes the synthesized authenticator:\n{out}",
    );
    assert!(out.contains("__auth.nil?"), "a missing record must not dispatch:\n{out}");
    assert!(out.contains("found = __auth"), "the consumer reads the bound record:\n{out}");
    assert!(diags.is_empty(), "grounded site should not produce residue: {diags:?}");
}

#[test]
fn relation_receiver_needs_no_class_method_delegation() {
    let (out, diags) = lower_and_emit(
        r#"
class Doorman
  def admit(email, secret)
    found = User.active.authenticate_by(email_address: email, password: secret)
    found
  end
end
"#,
    );
    assert!(!out.contains("authenticate_by"), "site should be inlined:\n{out}");
    assert!(
        out.contains("__auth = User.active.find_by(email_address: email)"),
        "find_by is Relation surface — the whole receiver chain is preserved:\n{out}",
    );
    assert!(diags.is_empty(), "grounded site should not produce residue: {diags:?}");
}

#[test]
fn assignment_in_a_condition_hoists_above_the_statement() {
    let (out, diags) = lower_and_emit(
        r#"
class Doorman
  def admit(email, secret)
    if user = User.active.authenticate_by(email_address: email, password: secret)
      user
    end
  end
end
"#,
    );
    let bind = out.find("__auth = User.active.find_by").expect(&format!("bind:\n{out}"));
    let cond = out.find("if user = __auth").expect(&format!("consumer:\n{out}"));
    assert!(bind < cond, "the expansion must precede the statement it fed:\n{out}");
    assert!(diags.is_empty(), "grounded site should not produce residue: {diags:?}");
}

#[test]
fn model_without_the_macro_stays_put_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Doorman
  def admit(title, secret)
    Article.authenticate_by(title: title, password: secret)
  end
end
"#,
    );
    assert!(out.contains("authenticate_by"), "no macro, nothing to expand:\n{out}");
    assert_eq!(diags.len(), 1, "the unresolved call goes on the ledger: {diags:?}");
    assert!(
        format!("{:?}", diags[0]).contains("does not declare has_secure_password"),
        "the ledger entry must name the gate that actually failed: {:?}",
        diags[0]
    );
}

#[test]
fn hash_with_no_finder_key_stays_put_with_residue() {
    // Rails itself raises ArgumentError here — a call that would raise
    // is not one to expand.
    let (out, diags) = lower_and_emit(
        r#"
class Doorman
  def admit(secret)
    User.authenticate_by(password: secret)
  end
end
"#,
    );
    assert!(out.contains("authenticate_by"), "site should be left alone:\n{out}");
    assert_eq!(diags.len(), 1, "the unresolved call goes on the ledger: {diags:?}");
    assert!(
        format!("{:?}", diags[0]).contains("no finder key"),
        "the ledger entry must name the gate that actually failed: {:?}",
        diags[0]
    );
}
