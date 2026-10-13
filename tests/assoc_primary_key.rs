//! `primary_key:` on `has_one` / `has_many` (`belongs_to`'s target column is
//! the belongs_to code's own)
//! (`lower::assoc_primary_key`): ingest records it, the synthesized
//! readers and builders key on it instead of `id`, and the paths that
//! cannot honor it refuse with a diagnostic.
//!
//! The behavior against real rows is `tests/emit_and_run/
//! assoc_primary_key.rs`; these pin the shapes.

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::Association;
use roundhouse::emit::ruby::{emit_library, emit_lowered_models};
use roundhouse::ingest::{ingest_library_classes, ingest_model, ingest_schema};
use roundhouse::lower::apply_has_one_builder_lowering;
use roundhouse::diagnostic::Severity;
use roundhouse::App;

const SCHEMA: &[u8] = br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.integer "source_id"
  end

  create_table "source_profiles", force: :cascade do |t|
    t.integer "source_id"
  end

  create_table "source_notes", force: :cascade do |t|
    t.integer "source_id"
  end
end
"#;

fn app_with(models: &[(&str, &str)], library: &str) -> App {
    let schema = ingest_schema(SCHEMA, "db/schema.rb").expect("ingest schema");
    let mut app = App::new();
    for (src, path) in models {
        let model = ingest_model(src.as_bytes(), path, &schema, &Default::default())
            .expect("ingest model")
            .expect("model recognized");
        app.models.push(model);
    }
    app.schema = schema;
    for lc in ingest_library_classes(library.as_bytes(), "test.rb").expect("ingest library") {
        app.library_classes.push(lc);
    }
    app
}

const ARTICLE: &str = "class Article < ApplicationRecord
  has_one :source_profile, foreign_key: :source_id, primary_key: :source_id
  has_many :source_notes, foreign_key: :source_id, primary_key: :source_id
  has_many :plain_notes, class_name: \"SourceNote\", foreign_key: :source_id
end
";
const PROFILE: &str = "class SourceProfile < ApplicationRecord\nend\n";
const NOTE: &str = "class SourceNote < ApplicationRecord\nend\n";

fn models() -> Vec<(&'static str, &'static str)> {
    vec![
        (ARTICLE, "app/models/article.rb"),
        (PROFILE, "app/models/source_profile.rb"),
        (NOTE, "app/models/source_note.rb"),
    ]
}

fn emit_models(app: &App) -> String {
    emit_lowered_models(app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ingest_records_the_primary_key_option() {
    let app = app_with(&models(), "");
    let article = &app.models[0];
    let keyed: Vec<(String, Option<String>)> = article
        .associations()
        .map(|a| (a.name().to_string(), a.declared_primary_key().map(|k| k.to_string())))
        .collect();
    assert_eq!(
        keyed,
        vec![
            ("source_profile".to_string(), Some("source_id".to_string())),
            ("source_notes".to_string(), Some("source_id".to_string())),
            ("plain_notes".to_string(), None),
        ]
    );
    let owner_keys: Vec<String> = article.associations().map(|a| a.owner_key().to_string()).collect();
    assert_eq!(owner_keys, ["source_id", "source_id", "id"]);
    assert!(matches!(article.associations().next(), Some(Association::HasOne { .. })));
}

#[test]
fn belongs_to_records_the_target_column() {
    let app = app_with(
        &[(
            "class SourceNote < ApplicationRecord\n  belongs_to :article, foreign_key: :source_id, primary_key: :source_id\nend\n",
            "app/models/source_note.rb",
        )],
        "",
    );
    let key = app.models[0].associations().next().and_then(|a| a.declared_primary_key().cloned());
    assert_eq!(key.map(|k| k.to_string()), Some("source_id".to_string()));
}

/// The text of `def <name>` up to the next method.
fn method_text<'a>(out: &'a str, name: &str) -> &'a str {
    let start = out.find(&format!("def {name}\n")).unwrap_or_else(|| panic!("no def {name}:\n{out}"));
    let rest = &out[start..];
    let end = rest[1..].find("\n  def ").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

#[test]
fn readers_key_on_the_owner_column_not_id() {
    let app = app_with(&models(), "");
    let out = emit_models(&app);
    for reader in ["source_profile", "source_notes"] {
        let body = method_text(&out, reader);
        assert!(body.contains("@source_id"), "{reader} must read the owner's source_id:\n{body}");
        assert!(!body.contains("@id"), "{reader} must not fall back to id:\n{body}");
        // a NULL owner key must match nothing (Rails), not the orphan rows
        assert!(body.contains("source_id IS NOT NULL"), "{reader} needs the nil-key guard:\n{body}");
    }
    // The unkeyed sibling still matches on id, so the rule is per association.
    let plain = method_text(&out, "plain_notes");
    assert!(plain.contains("@id") && !plain.contains("@source_id"), "{plain}");
    assert!(!plain.contains("IS NOT NULL"), "{plain}");
}

#[test]
fn has_one_builders_preset_the_foreign_key_from_the_owner_column() {
    let mut app = app_with(
        &models(),
        "class Maker\n  def make(id)\n    article = Article.find(id)\n    article.create_source_profile!\n  end\nend\n",
    );
    Analyzer::new(&app).analyze(&mut app);
    apply_has_one_builder_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        out.contains("source_id: article.source_id"),
        "create_source_profile! must stamp the owner's source_id, not article.id:\n{out}"
    );
    assert!(!out.contains("source_id: article.id"), "{out}");
}

fn refusals(models: &[(&str, &str)]) -> Vec<String> {
    let app = app_with(models, "");
    let (_, diags) = roundhouse::emit::diagnostics::scope(|| emit_models(&app));
    diags
        .into_iter()
        .filter(|d| d.severity == Severity::Error && d.message.contains("primary_key"))
        .map(|d| d.message)
        .collect()
}

#[test]
fn keyed_direct_associations_do_not_refuse() {
    assert!(refusals(&models()).is_empty());
}

#[test]
fn a_through_hop_keyed_on_a_column_refuses() {
    let out = refusals(&[
        (
            "class Article < ApplicationRecord\n  has_many :source_notes, foreign_key: :source_id, primary_key: :source_id\n  has_many :profiles, through: :source_notes, source: :source_profile\nend\n",
            "app/models/article.rb",
        ),
        (
            "class SourceNote < ApplicationRecord\n  belongs_to :source_profile\nend\n",
            "app/models/source_note.rb",
        ),
        (PROFILE, "app/models/source_profile.rb"),
    ]);
    assert!(out.iter().any(|m| m.contains("Article.profiles")), "{out:?}");
}

/// The refused association must not keep a reader: a diagnostic beside a
/// direct `Target.where(fk: owner_key)` reader would still ship the rows.
fn refused_models(models: &[(&str, &str)]) -> (Vec<String>, String) {
    let app = app_with(models, "");
    let (out, diags) = roundhouse::emit::diagnostics::scope(|| emit_models(&app));
    let msgs = diags
        .into_iter()
        .filter(|d| d.severity == Severity::Error && d.message.contains("primary_key"))
        .map(|d| d.message)
        .collect();
    (msgs, out)
}

#[test]
fn a_through_chain_too_deep_to_check_refuses_and_keeps_no_reader() {
    // h0 is keyed; h6 reaches it through six hops, past the depth the
    // chain resolver (and this check) will follow.
    let mut article = String::from(
        "class Article < ApplicationRecord\n  has_many :h0, class_name: \"SourceNote\", foreign_key: :source_id, primary_key: :source_id\n",
    );
    for i in 1..=6 {
        article.push_str(&format!(
            "  has_many :h{i}, through: :h{}, class_name: \"SourceNote\"\n",
            i - 1
        ));
    }
    article.push_str("end\n");
    let (msgs, out) = refused_models(&[
        (&article, "app/models/article.rb"),
        (NOTE, "app/models/source_note.rb"),
    ]);
    assert!(
        msgs.iter().any(|m| m.contains("Article.h6") && m.contains("deeper than")),
        "{msgs:?}"
    );
    assert!(!out.contains("def h6\n"), "the refused through must not keep a reader:\n{out}");
}

#[test]
fn a_keyed_has_one_through_refuses_and_keeps_no_reader() {
    let (msgs, out) = refused_models(&[
        (
            "class Article < ApplicationRecord\n  has_many :source_notes, foreign_key: :source_id\n  has_one :source_profile, through: :source_notes, primary_key: :source_id\nend\n",
            "app/models/article.rb",
        ),
        (PROFILE, "app/models/source_profile.rb"),
        (NOTE, "app/models/source_note.rb"),
    ]);
    assert!(msgs.iter().any(|m| m.contains("Article.source_profile") && m.contains("has_one")), "{msgs:?}");
    assert!(!out.contains("def source_profile\n"), "{out}");
}

#[test]
fn an_unkeyed_has_one_through_is_untouched() {
    // Not this change's concern: main lowers it as a direct query, and
    // it still does.
    let (msgs, out) = refused_models(&[
        (
            "class Article < ApplicationRecord\n  has_many :source_notes, foreign_key: :source_id\n  has_one :source_profile, through: :source_notes\nend\n",
            "app/models/article.rb",
        ),
        (PROFILE, "app/models/source_profile.rb"),
        (NOTE, "app/models/source_note.rb"),
    ]);
    assert!(msgs.is_empty(), "{msgs:?}");
    assert!(out.contains("def source_profile\n"));
}
