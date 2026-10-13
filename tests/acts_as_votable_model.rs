//! `acts_as_votable` (the gem): `ActsAsVotable::Vote` resolves as a model on
//! `votes` and the macros expand, when the gem is locked and the table is
//! present; otherwise the constant stays unresolved and the macros refuse.

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::dialect::ModelBodyItem;
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    acts_as_votable (0.14.0)\n      activerecord (>= 4.2)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  acts_as_votable\n";

fn tree(lock: Option<&str>, schema: &str, models: &[(&str, &str)]) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
    let mut files: Vec<(String, String)> = vec![("db/schema.rb".into(), schema.into())];
    if let Some(lock) = lock {
        files.push(("Gemfile.lock".into(), lock.into()));
    }
    for (name, src) in models {
        files.push((format!("app/models/{name}.rb"), (*src).into()));
    }
    files.into_iter().map(|(p, c)| (p.into(), c.into_bytes())).collect()
}

const VOTES: &str = r#"  create_table :votes do |t|
    t.string :votable_type
    t.integer :votable_id
    t.string :voter_type
    t.integer :voter_id
    t.boolean :vote_flag
    t.string :vote_scope
    t.integer :vote_weight
    t.timestamps
  end
"#;

fn schema(with_votes: bool) -> String {
    format!(
        "ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n    t.integer :cached_votes_up, default: 0\n    t.integer :cached_votes_total, default: 0\n    t.integer :cached_scoped_flag_votes_up, default: 0\n  end\n  create_table :users do |t|\n    t.string :name\n  end\n{}end\n",
        if with_votes { VOTES } else { "" }
    )
}

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";
const ARTICLE: &str = r#"class Article < ApplicationRecord
  acts_as_votable

  def self.upvoted_ids(user)
    ActsAsVotable::Vote.where(voter: user, vote_flag: true, votable_type: "Article").pluck(:votable_id)
  end

  def likes_by(user)
    liked_by(user)
    cached_votes_up
  end

  def flag(user)
    vote_by(voter: user, vote_scope: "flag")
  end

  def undo(user)
    unliked_by(user)
  end
end
"#;
const USER: &str = r#"class User < ApplicationRecord
  acts_as_voter

  def liked_article?(article)
    voted_up_on?(article)
  end

  def vote_count
    votes.count
  end
end
"#;

fn errors(app: &mut roundhouse::App) -> Vec<String> {
    let lower = roundhouse::session::analyze_and_lower(app);
    diagnose(app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} @{:?}", d.message, d.span))
        .collect()
}

fn models() -> [(&'static str, &'static str); 3] {
    [("application_record", APPLICATION_RECORD), ("article", ARTICLE), ("user", USER)]
}

#[test]
fn vote_model_and_macros_resolve() {
    let mut app = ingest_app_from_tree(tree(Some(LOCK), &schema(true), &models())).expect("ingest");
    let errs = errors(&mut app);
    assert!(errs.is_empty(), "{errs:?}");

    let vote = app.models.iter().find(|m| m.name.0.as_str() == "ActsAsVotable::Vote").expect("Vote model");
    assert_eq!(vote.table.0.as_str(), "votes");
    let assocs = |name: &str| -> Vec<String> {
        app.models
            .iter()
            .find(|m| m.name.0.as_str() == name)
            .unwrap()
            .body
            .iter()
            .filter_map(|i| match i {
                ModelBodyItem::Association { assoc, .. } => Some(assoc.name().as_str().to_string()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(assocs("ActsAsVotable::Vote"), ["votable", "voter"]);
    assert_eq!(assocs("Article"), ["votes_for"]);
    assert_eq!(assocs("User"), ["votes"]);
    // The macro is the expansion, not an extra unknown statement.
    for name in ["Article", "User"] {
        let m = app.models.iter().find(|m| m.name.0.as_str() == name).unwrap();
        assert!(!m.body.iter().any(|i| matches!(i, ModelBodyItem::Unknown { .. })), "{name}");
    }
}

#[test]
fn cached_updates_follow_the_columns_the_table_has() {
    let app = ingest_app_from_tree(tree(Some(LOCK), &schema(true), &models())).unwrap();
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    let body = article
        .body
        .iter()
        .find_map(|i| match i {
            ModelBodyItem::Method { method, .. } if method.name.as_str() == "update_cached_votes" => {
                Some(format!("{:?}", method.body))
            }
            _ => None,
        })
        .expect("update_cached_votes");
    for col in ["cached_votes_up", "cached_votes_total", "cached_scoped_flag_votes_up"] {
        assert!(body.contains(col), "{col} missing");
    }
    for col in ["cached_votes_down", "cached_votes_score", "cached_weighted_total"] {
        assert!(!body.contains(col), "{col} must not be written (no such column)");
    }
}

#[test]
fn without_the_gem_the_constant_stays_unresolved() {
    let mut app = ingest_app_from_tree(tree(None, &schema(true), &[("application_record", APPLICATION_RECORD), ("article", &ARTICLE.replace("  acts_as_votable\n", ""))])).unwrap();
    assert!(!app.models.iter().any(|m| m.name.0.as_str() == "ActsAsVotable::Vote"));
    let errs = errors(&mut app);
    assert!(errs.iter().any(|e| e.contains("ActsAsVotable::Vote")), "{errs:?}");
}

#[test]
fn without_the_votes_table_the_constant_stays_unresolved() {
    let mut app = ingest_app_from_tree(tree(Some(LOCK), &schema(false), &[("application_record", APPLICATION_RECORD), ("article", &ARTICLE.replace("  acts_as_votable\n", ""))])).unwrap();
    assert!(!app.models.iter().any(|m| m.name.0.as_str() == "ActsAsVotable::Vote"));
    let errs = errors(&mut app);
    assert!(errs.iter().any(|e| e.contains("ActsAsVotable::Vote")), "{errs:?}");
}

#[test]
fn an_unmodeled_strategy_is_refused() {
    let src = ARTICLE.replace("acts_as_votable\n", "acts_as_votable cacheable_strategy: :nope\n");
    let result = ingest_app_from_tree(tree(Some(LOCK), &schema(true), &[("application_record", APPLICATION_RECORD), ("article", &src)]));
    let msg = match result {
        Ok(_) => String::new(),
        Err(e) => format!("{e:?}"),
    };
    assert!(msg.contains("cacheable_strategy"), "{msg}");
}

#[test]
fn repeated_macros_with_different_options_are_refused_and_identical_ones_collapse() {
    let differing = ARTICLE.replace(
        "  acts_as_votable\n",
        "  acts_as_votable\n  acts_as_votable cacheable_strategy: :update_columns\n",
    );
    let err = ingest_app_from_tree(tree(
        Some(LOCK),
        &schema(true),
        &[("application_record", APPLICATION_RECORD), ("article", &differing)],
    ))
    .err()
    .expect("refused");
    assert!(err.to_string().contains("different options"), "{err}");

    let same = ARTICLE.replace("  acts_as_votable\n", "  acts_as_votable\n  acts_as_votable\n");
    let app = ingest_app_from_tree(tree(
        Some(LOCK),
        &schema(true),
        &[("application_record", APPLICATION_RECORD), ("article", &same)],
    ))
    .expect("identical repeats are one expansion");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    let n = article
        .body
        .iter()
        .filter(|i| matches!(i, ModelBodyItem::Association { assoc, .. } if assoc.name().as_str() == "votes_for"))
        .count();
    assert_eq!(n, 1);
}
