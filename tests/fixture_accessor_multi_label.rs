//! `users(:david, :jason)` — Rails' fixture accessor is `def
//! users(*names)`, and with more than one name it answers the ARRAY of
//! those records.
//!
//! The rewrite that binds `users(:david)` to `UsersFixtures.david`
//! gated on `args.len() == 1`, so a two-name call fell through
//! unrewritten and survived into the emitted test as a bare
//! `users(:david, :jason)` — `undefined method 'users' for an instance
//! of AccountsControllerTest`, which is the same silent hole the
//! value-label form (`users(name)`) was opened to close, one arity over.
//!
//! An Array LITERAL of the per-label readers, not a variadic helper:
//! each element is the concrete dispatch the single-name form already
//! emits, so the element type follows from the members and no target
//! needs a splat.

use std::collections::HashMap;
use std::path::PathBuf;

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

const FIXTURES: &str = r#"david:
  name: David

jason:
  name: Jason
"#;

const TEST: &str = r#"require "test_helper"

class UserTest < ActiveSupport::TestCase
  test "names" do
    one = users(:david).name
    both = users(:david, :jason).map(&:name)
    assert_equal [ one ], both.first(1)
  end
end
"#;

/// The lowered test body, as IR debug text — `rewrite_fixture_calls`
/// runs inside `lower_test_modules_to_library_classes`, so this is the
/// pass's own output rather than a re-derivation of it.
fn lowered_test_body() -> String {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("test/fixtures/users.yml", FIXTURES),
        ("test/models/user_test.rb", TEST),
    ]))
    .expect("ingest");
    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
    );
    lcs.iter()
        .flat_map(|lc| lc.methods.iter())
        .map(|m| format!("{:?}", m.body))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn several_labels_answer_an_array_of_the_readers() {
    let body = lowered_test_body();
    assert!(
        body.contains("Array"),
        "the multi-name call answers an Array node:\n{body}"
    );
    for label in ["david", "jason"] {
        assert!(
            body.contains(&format!("Symbol(\"{label}\")")),
            "`{label}` is bound to its own reader:\n{body}"
        );
    }
    assert!(
        body.contains("UsersFixtures"),
        "the readers dispatch on the fixture class:\n{body}"
    );
}

/// Nothing defines a bare `users(...)` anywhere in a target tree, so
/// an unrewritten accessor is a NameError waiting to run — which is
/// exactly how the two-name form announced itself.
#[test]
fn no_bare_accessor_survives() {
    let body = lowered_test_body();
    assert!(
        !body.contains("method: Symbol(\"users\")"),
        "every accessor call is rewritten:\n{body}"
    );
}
