//! Shared test-support modules — `test/test_helpers/*.rb`.
//!
//! Rails apps put cross-cutting test helpers in modules and mix them
//! into `ActiveSupport::TestCase` from `test/test_helper.rb`. campfire
//! does exactly that, and until this landed the single method
//! `SessionTestHelper#sign_in` was the FIRST failure in 25 of its 52
//! emitted test files — nearly half the suite blocked on one `def`
//! nobody had read.
//!
//! Spliced onto each test class rather than emitted as an `include`:
//! a test class's `helpers` already lower to ordinary instance methods
//! on it, which is what the mixin means, and it asks nothing of a
//! target's mixin semantics — the same call `splice_concerns_into_models`
//! made.

use roundhouse::app::App;

/// A campfire-shaped app: a helper module the app's `test_helper.rb`
/// mixes in, a second one it does NOT, and a test that calls into the
/// first.
fn app_with_test_helpers() -> App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    t.string :email_address\n  end\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "test/test_helper.rb",
            "class ActiveSupport::TestCase\n  \
             include SessionTestHelper\n\
             end\n",
        ),
        (
            "test/test_helpers/session_test_helper.rb",
            "module SessionTestHelper\n  \
             def sign_in(user)\n    \
             user = users(user)\n    \
             user.email_address\n  end\n  \
             def parsed_cookies\n    \
             ActionDispatch::Cookies::CookieJar.build(request, cookies.to_hash)\n  \
             end\nend\n",
        ),
        // Included by `application_system_test_case.rb`, NOT by
        // test_helper.rb — must not be spliced.
        (
            "test/test_helpers/system_test_helper.rb",
            "module SystemTestHelper\n  \
             def visit_root\n    visit(root_url)\n  end\nend\n",
        ),
        (
            "test/models/user_test.rb",
            "class UserTest < ActiveSupport::TestCase\n  \
             test \"signs in\" do\n    sign_in(:one)\n  end\n  \
             test \"reads a cookie\" do\n    \
             assert parsed_cookies.signed[:session_token]\n  end\nend\n",
        ),
        ("test/fixtures/users.yml", "one:\n  email_address: a@example.com\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

fn helper_names(app: &App) -> Vec<String> {
    app.test_modules
        .iter()
        .flat_map(|tm| tm.helpers.iter())
        .map(|h| h.name.as_str().to_string())
        .collect()
}

#[test]
fn an_included_helper_module_is_spliced_onto_the_test_class() {
    // The regression this file exists for: before the splice,
    // `sign_in` existed nowhere in the emit and every test that called
    // it raised NoMethodError on the first line.
    let app = app_with_test_helpers();
    assert!(
        helper_names(&app).contains(&"sign_in".to_string()),
        "expected sign_in spliced; got {:?}",
        helper_names(&app),
    );
}

#[test]
fn a_module_the_app_does_not_include_is_not_spliced() {
    // The filter is what keeps `SystemTestHelper` — Capybara all the
    // way down — out of every test class. Globbing the directory would
    // have put a pile of permanently-unresolvable dispatch into the
    // emit for methods nothing can reach (`test/system/` is not
    // ingested).
    let app = app_with_test_helpers();
    assert!(
        !helper_names(&app).contains(&"visit_root".to_string()),
        "SystemTestHelper is included only by the system-test base and \
         must not be spliced; got {:?}",
        helper_names(&app),
    );
}

#[test]
fn a_spliced_helper_gets_the_fixture_and_route_rewrites() {
    // A helper body is app code like any other: `users(user)` has to
    // reach the fixture class and `session_url` the route helpers.
    // Both rewrites used to run on test METHODS only, so a spliced
    // helper arrived with bare Sends that resolve to nothing —
    // `sign_in`'s two walls, in order, after the splice itself.
    let app = app_with_test_helpers();
    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
    );
    let body = lcs
        .iter()
        .flat_map(|lc| lc.methods.iter())
        .find(|m| m.name.as_str() == "sign_in")
        .map(|m| format!("{:?}", m.body))
        .expect("lowered sign_in");
    assert!(
        body.contains("UsersFixtures"),
        "expected the fixture rewrite in sign_in's body:\n{body}",
    );
}

#[test]
fn a_variable_fixture_label_routes_through_by_label() {
    // `users(:one)` binds to the generated reader; `users(name)` cannot
    // — and Rails allows both. campfire's `sign_in` writes the second,
    // so the fixture class carries a compile-time label table for it.
    let app = app_with_test_helpers();
    let fixture_lcs = roundhouse::lower::lower_fixtures_to_library_classes(&app);
    let users = fixture_lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "UsersFixtures")
        .expect("UsersFixtures");
    assert!(
        users.methods.iter().any(|m| m.name.as_str() == "by_label"),
        "expected a by_label table on UsersFixtures",
    );

    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
    );
    let body = lcs
        .iter()
        .flat_map(|lc| lc.methods.iter())
        .find(|m| m.name.as_str() == "sign_in")
        .map(|m| format!("{:?}", m.body))
        .expect("lowered sign_in");
    assert!(
        body.contains("by_label"),
        "a variable label must route through by_label:\n{body}",
    );
}

#[test]
fn a_test_classes_own_definition_wins_over_the_module() {
    // Ruby resolves the class body ahead of an included module.
    let files: Vec<(&str, &str)> = vec![
        (
            "test/test_helper.rb",
            "class ActiveSupport::TestCase\n  include SessionTestHelper\nend\n",
        ),
        (
            "test/test_helpers/session_test_helper.rb",
            "module SessionTestHelper\n  def sign_in(u)\n    :from_module\n  end\nend\n",
        ),
        (
            "test/models/user_test.rb",
            "class UserTest < ActiveSupport::TestCase\n  \
             def sign_in(u)\n    :from_class\n  end\n  \
             test \"x\" do\n    sign_in(1)\n  end\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");

    let signs: Vec<&roundhouse::dialect::MethodDef> = app
        .test_modules
        .iter()
        .flat_map(|tm| tm.helpers.iter())
        .filter(|h| h.name.as_str() == "sign_in")
        .collect();
    assert_eq!(signs.len(), 1, "exactly one sign_in survives");
    assert!(
        format!("{:?}", signs[0].body).contains("from_class"),
        "the test class's own definition must win",
    );
}

#[test]
fn a_spliced_helper_with_no_signature_returns_what_its_body_returns() {
    // A helper ingest attaches no signature to was typed `-> nil`, and
    // the test registry copied that in — so campfire's
    // `parsed_cookies.signed[:session_token]` typed nil at the receiver
    // and `[]` behind it was left to dispatch on a boxed value, which
    // the compiled lane refuses at runtime (five tests, four files).
    // The body has the type once it is typed: lift it, the way an
    // inner stand-in's return is lifted.
    let app = app_with_test_helpers();
    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
    );
    let methods: Vec<_> = lcs.iter().flat_map(|lc| lc.methods.iter()).collect();
    let sig = methods
        .iter()
        .find(|m| m.name.as_str() == "parsed_cookies")
        .and_then(|m| m.signature.as_ref())
        .map(|t| format!("{t:?}"))
        .expect("lowered parsed_cookies with a signature");
    assert!(
        sig.contains("ActionController::CookieJar"),
        "expected parsed_cookies to return the jar its body builds, got {sig}",
    );
    // And the reader that was the actual wall: `signed` resolves on the
    // lifted type, so the test body's `[]` has a typed receiver.
    let body = methods
        .iter()
        .find(|m| m.name.as_str() == "test_reads_a_cookie")
        .map(|m| format!("{:?}", m.body))
        .expect("lowered test_reads_a_cookie");
    assert!(
        body.contains("ActionController::SignedCookieJar"),
        "expected the signed jar typed in the test body:\n{body}",
    );
}
