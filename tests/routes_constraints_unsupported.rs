//! CodeRabbit on rubys/roundhouse#1087 (which added `constraints:` on
//! `resources`/`resource`, #857): two gaps in the per-param regex
//! reader `regex_source` shares between `resources`/`resource` and an
//! explicit route (`get`/`post`/…).
//!
//! 1. A `constraints:` value that isn't a literal regex (a method
//!    call, a constant, a bare string, …) made `regex_source` return
//!    `None`; the reader then just skipped that param with no
//!    diagnostic, so the route lowered with no constraint on it at
//!    all. A member route could then accept a sibling collection path
//!    with nothing refusing it. Fixed: an unparseable value is now a
//!    located `IngestError::Unsupported` gap.
//!
//! 2. `regex_source` returns only the pattern body between the
//!    delimiters, dropping any modifier (`/i`, `/x`, `/m`). A
//!    constraint like `/[a-z]+/i` would therefore lower identically to
//!    `/[a-z]+/`, matching a different set of paths than Rails does.
//!    Minimum fix in this PR: a regex constraint carrying a modifier is
//!    refused as a located `Unsupported` gap instead of being lowered
//!    without it. Full modifier support (plumbing the flag through
//!    `lower::routes`'s segment-pattern compiler) is a follow-up.
//!
//! Both gaps are exercised on all three readers that share
//! `read_param_regex_constraints`: `resources`/`resource`'s inline
//! kwarg, an explicit route's `constraints:` kwarg, and the
//! `constraints(...) do … end` block form.

use roundhouse::dialect::RouteTable;
use roundhouse::ingest::{ingest_routes, IngestError};

fn ingest(source: &str) -> Result<RouteTable, IngestError> {
    ingest_routes(format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(), "config/routes.rb")
}

fn expect_unsupported(source: &str, needle: &str) {
    match ingest(source) {
        Ok(_) => panic!("expected an Unsupported ingest error for {source:?}, got Ok"),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains(needle),
                "error message {msg:?} does not mention {needle:?} (source: {source:?})"
            );
        }
    }
}

// --- Finding 1: an unparseable constraints: value ------------------

#[test]
fn resources_constraints_with_an_unparseable_value_is_a_located_gap() {
    expect_unsupported(
        r#"resources :widgets, constraints: { id: ID_PATTERN }"#,
        "is not a literal regex",
    );
}

#[test]
fn explicit_route_constraints_with_an_unparseable_value_is_a_located_gap() {
    expect_unsupported(
        r#"get "/widgets/:id", to: "widgets#show", constraints: { id: ID_PATTERN }"#,
        "is not a literal regex",
    );
}

#[test]
fn block_constraints_with_an_unparseable_value_is_a_located_gap() {
    expect_unsupported(
        r#"constraints(id: ID_PATTERN) do
  get "/widgets/:id", to: "widgets#show"
end"#,
        "is not a literal regex",
    );
}

// --- Finding 2: a regex constraint carrying a modifier --------------

#[test]
fn resources_constraints_regex_with_a_modifier_is_refused() {
    expect_unsupported(r#"resources :widgets, constraints: { id: /[a-z]+/i }"#, "regex modifiers");
}

#[test]
fn explicit_route_constraints_regex_with_a_modifier_is_refused() {
    expect_unsupported(
        r#"get "/widgets/:id", to: "widgets#show", constraints: { id: /[a-z]+/i }"#,
        "regex modifiers",
    );
}

#[test]
fn block_constraints_regex_with_a_modifier_is_refused() {
    expect_unsupported(
        r#"constraints(id: /[a-z]+/i) do
  get "/widgets/:id", to: "widgets#show"
end"#,
        "regex modifiers",
    );
}

// --- Control: a plain regex constraint (no modifier) still works ----

#[test]
fn resources_constraints_plain_regex_still_lowers() {
    let table = ingest(r#"resources :widgets, constraints: { id: /[a-z]+/ }"#).expect("plain regex constraint must ingest");
    assert!(!table.entries.is_empty(), "resources call must still produce routes");
}

#[test]
fn explicit_route_constraints_plain_regex_still_lowers() {
    let table = ingest(r#"get "/widgets/:id", to: "widgets#show", constraints: { id: /[a-z]+/ }"#)
        .expect("plain regex constraint must ingest");
    assert!(!table.entries.is_empty(), "explicit route must still ingest");
}

#[test]
fn block_constraints_plain_regex_still_lowers() {
    let table = ingest(
        r#"constraints(id: /[a-z]+/) do
  get "/widgets/:id", to: "widgets#show"
end"#,
    )
    .expect("plain regex block constraint must ingest");
    assert!(!table.entries.is_empty(), "block constraints must still produce routes");
}

// --- Survey recovery: block fail-closed must not zero sibling routes ----

#[test]
fn survey_mode_keeps_siblings_when_block_constraints_are_unsupported() {
    use roundhouse::ingest::{prism, survey};

    survey::activate();
    let (result, _) = prism::scope(|| {
        ingest(
            r#"
get "/ok", to: "widgets#ok"
constraints(id: ID_PATTERN) do
  get "/widgets/:id", to: "widgets#show"
end
get "/also", to: "widgets#also"
"#,
        )
    });
    let gaps = survey::drain();
    let table = result.expect("survey ingest must keep walking after a bad constraints block");
    assert_eq!(gaps.len(), 1, "exactly one survey gap: {gaps:?}");
    assert!(
        gaps.iter().any(|g| g.to_string().contains("is not a literal regex")),
        "gap must name the unparseable value: {gaps:?}"
    );
    // Two sibling gets outside the block; the block's child is dropped.
    assert_eq!(
        table.entries.len(),
        2,
        "siblings outside the bad block must survive: {:?}",
        table.entries
    );
}
