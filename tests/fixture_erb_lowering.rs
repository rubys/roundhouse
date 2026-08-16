//! ERB-templated YAML fixtures (`ingest::fixture` + `lower::fixtures`).
//!
//! Rails renders every fixture file through ERB before YAML sees it,
//! and ingest used to drop any file containing a tag — "only knowable
//! to a running Ruby." That reasoning holds for the *value*, not for
//! the *program*: the emitted loader IS Ruby, so a tag can ride through
//! as an expression and evaluate where Rails would have evaluated it.
//!
//! campfire is the forcing function. Its `users.yml`, `messages.yml`
//! and `sessions.yml` are the only three fixture files it has with ERB
//! — and they are exactly the three every one of its 63 test files
//! needs (signing in needs a user, half the model tests reference
//! `messages(:third)`). Between them they cover the whole surface this
//! file pins: a statement tag binding a local, a value tag reading it
//! back, a relative time, and a call into the app's own model.

use roundhouse::app::App;
use roundhouse::dialect::FixtureValue;
use roundhouse::Symbol;

/// A campfire-shaped app: one model whose columns cover each ERB value
/// kind, plus the fixture file under test.
fn app_with_fixture(fixture_yml: &str) -> App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    \
             t.string :name\n    \
             t.string :password_digest\n    \
             t.string :bot_token\n    \
             t.datetime :created_at\n  \
             end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
             def self.generate_bot_token\n    \
             \"tok\"\n  end\nend\n",
        ),
        (
            "test/fixtures/users.yml",
            Box::leak(fixture_yml.to_string().into_boxed_str()),
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

/// The emitted `test/fixtures/users.rb` — the loader a test run
/// actually executes.
fn emitted_fixture(app: &App) -> String {
    roundhouse::emit::ruby::emit_spinel(app)
        .into_iter()
        .find(|f| f.path.to_string_lossy() == "test/fixtures/users.rb")
        .map(|f| f.content)
        .expect("test/fixtures/users.rb")
}

/// campfire's `users.yml`, trimmed to two records. All three tag kinds.
const CAMPFIRE_USERS: &str = r#"<% password_digest = BCrypt::Password.create("secret123456") %>

david:
  name: David
  password_digest: <%= password_digest %>
  created_at: <%= 1.hour.ago %>

bender:
  name: Bender
  password_digest: <%= password_digest %>
  bot_token: <%= User.generate_bot_token %>
"#;

// ── ingest ──────────────────────────────────────────────────────────

#[test]
fn an_erb_fixture_is_ingested_rather_than_dropped() {
    // The regression this whole file exists for: before ERB lowering,
    // `app.fixtures` came back EMPTY for a file like this and the only
    // trace was a ledger line.
    let app = app_with_fixture(CAMPFIRE_USERS);
    let users = app
        .fixtures
        .iter()
        .find(|f| f.name.as_str() == "users")
        .expect("users fixture ingested");
    assert_eq!(users.records.len(), 2, "both labels survive the ERB split");
}

#[test]
fn a_statement_tag_becomes_the_fixtures_preamble() {
    // `<% password_digest = … %>` produces no output — it BINDS, and
    // four records downstream read the binding. It has to run once,
    // ahead of the inserts, which is what `preamble` is for.
    let app = app_with_fixture(CAMPFIRE_USERS);
    let users = &app.fixtures[0];
    assert_eq!(users.preamble.len(), 1, "one statement tag");
}

#[test]
fn a_value_tag_stays_an_expression_and_a_scalar_stays_a_scalar() {
    // The distinction that makes the rest work: `name: David` is data
    // the compiler knows, `password_digest: <%= … %>` is a program it
    // doesn't. Folding the second into a string at ingest would bake
    // one run's bcrypt digest (or one run's clock) into the emit.
    let app = app_with_fixture(CAMPFIRE_USERS);
    let david = app.fixtures[0]
        .records
        .get(&Symbol::from("david"))
        .expect("david");

    assert_eq!(
        david.get(&Symbol::from("name")).and_then(|v| v.as_scalar()),
        Some("David"),
    );
    assert!(matches!(
        david.get(&Symbol::from("password_digest")),
        Some(FixtureValue::Ruby(_)),
    ));
    assert!(matches!(
        david.get(&Symbol::from("created_at")),
        Some(FixtureValue::Ruby(_)),
    ));
}

#[test]
fn a_fixture_without_erb_is_untouched() {
    // The blog's shape. Every value is a scalar and there is no
    // preamble — this path must not change.
    let app = app_with_fixture("one:\n  name: Plain\n");
    let users = &app.fixtures[0];
    assert!(users.preamble.is_empty());
    assert_eq!(
        users.records[&Symbol::from("one")]
            .get(&Symbol::from("name"))
            .and_then(|v| v.as_scalar()),
        Some("Plain"),
    );
}

#[test]
fn a_tag_interpolated_into_a_larger_scalar_is_reported_not_guessed() {
    // `name: "hi <%= 1 %> there"` is a string BUILT at runtime, not the
    // tag's own result. Reachable Rails; nothing we ingest writes it.
    // Name the field rather than inventing a concatenation — under a
    // survey (`--allow-unsupported`) this is a ledger line and the file
    // drops; without one it is an ingest error, which is how every other
    // unsupported fixture shape already behaves.
    let tree = vec![(
        std::path::PathBuf::from("test/fixtures/users.yml"),
        b"one:\n  name: \"hi <%= 1 %> there\"\n".to_vec(),
    )]
    .into_iter()
    .collect();
    let err = roundhouse::ingest::ingest_app_from_tree(tree)
        .expect_err("an embedded tag is not silently mangled");
    assert!(
        err.to_string().contains("interpolated into a larger scalar"),
        "expected the field named in: {err}",
    );
}

// ── emit (ruby) ─────────────────────────────────────────────────────

#[test]
fn the_preamble_runs_ahead_of_the_inserts_in_the_loader() {
    // Ordering is the correctness property: `password_digest` is a
    // local, so its binding must lexically precede every read of it
    // inside the same method body.
    let out = emitted_fixture(&app_with_fixture(CAMPFIRE_USERS));
    let bind = out
        .find("password_digest = BCrypt::Password.create(\"secret123456\")")
        .expect("preamble assignment emitted");
    let read = out
        .find("instance.password_digest = password_digest")
        .expect("value tag read emitted");
    assert!(bind < read, "binding must precede the read:\n{out}");
}

#[test]
fn a_relative_time_tag_grounds_to_a_duration() {
    // `1.hour.ago` reaches emit ungrounded — fixture classes are
    // synthesized after the shared post-analyze hooks have run over the
    // App — so `emit::ruby` re-applies the duration rewrite to them.
    // Without it this emits a bare `1.hour`, which no target's runtime
    // answers.
    let out = emitted_fixture(&app_with_fixture(CAMPFIRE_USERS));
    assert!(
        out.contains("ActiveSupport::Duration"),
        "expected a grounded Duration in:\n{out}",
    );
}

#[test]
fn a_tag_calling_the_apps_own_model_emits_as_that_call() {
    // `<%= User.generate_bot_token %>` is a call into emitted code —
    // the fixture loader and the model land in the same tree, so this
    // needs nothing beyond ordinary dispatch.
    let out = emitted_fixture(&app_with_fixture(CAMPFIRE_USERS));
    assert!(
        out.contains("User.generate_bot_token"),
        "expected the model call in:\n{out}",
    );
}
