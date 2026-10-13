//! Conditionals in a `config/routes.rb` draw block (#145).
//!
//! `if` / `unless`, including modifier `if`, are walked: both arms stay
//! in the table so every environment's helpers exist. The predicate is
//! not evaluated. A `case` / `case in` is still not a call and is not
//! walked; it used to vanish on a bare `continue`. Strict ingest fails,
//! survey mode records a gap naming the construct and its line, and the
//! sibling routes still ingest.

use std::collections::HashMap;

use roundhouse::ingest::routes::ingest_routes_with_draws;
use roundhouse::ingest::{survey, IngestError};
use roundhouse::lower::flatten_routes;
use roundhouse::App;

const IF_BLOCK: &[u8] = b"Rails.application.routes.draw do\n  root \"pages#home\"\n  if Rails.env.development?\n    get \"/debug\", to: \"debug#show\"\n  end\nend\n";

fn ingest(source: &[u8]) -> Result<roundhouse::dialect::RouteTable, IngestError> {
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &HashMap::new())
    });
    result
}

fn paths_of(table: roundhouse::dialect::RouteTable) -> Vec<String> {
    let mut app = App::default();
    app.routes = table;
    flatten_routes(&app).into_iter().map(|r| r.path).collect()
}

/// Survey-mode ingest of `source`: the flattened paths and the gaps.
fn survey_ingest(source: &[u8]) -> (Vec<String>, Vec<IngestError>) {
    survey::activate();
    let result = ingest(source);
    let gaps = survey::drain();
    let paths = paths_of(result.expect("survey mode recovers the sibling routes"));
    (paths, gaps)
}

fn gap_messages(gaps: &[IngestError]) -> Vec<String> {
    gaps.iter().map(|g| g.to_string()).collect()
}

#[test]
fn an_if_route_block_keeps_both_arms() {
    let paths = paths_of(ingest(IF_BLOCK).expect("an if block is walked, not dropped"));
    assert!(
        paths.iter().any(|p| p == "/"),
        "the sibling root stays: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "/debug"),
        "the route inside the if stays, predicate unevaluated: {paths:?}"
    );

    let (paths, gaps) = survey_ingest(IF_BLOCK);
    assert!(
        paths.iter().any(|p| p == "/debug"),
        "survey keeps the if arm: {paths:?}"
    );
    let messages = gap_messages(&gaps);
    assert!(
        !messages.iter().any(|m| m.contains("conditional")),
        "a walked if is not a gap: {messages:?}"
    );
}

#[test]
fn an_unless_block_and_a_modifier_if_route_are_kept() {
    let source = b"Rails.application.routes.draw do\n  root \"pages#home\"\n  unless ENV[\"FEATURE_OFF\"]\n    get \"/feature\", to: \"feature#show\"\n  end\n  get \"/beta\", to: \"beta#show\" if ENV[\"BETA\"]\nend\n";
    let paths = paths_of(ingest(source).expect("unless and modifier if are walked"));
    assert!(
        paths.iter().any(|p| p == "/feature") && paths.iter().any(|p| p == "/beta"),
        "both arms stay: {paths:?}"
    );

    let (paths, gaps) = survey_ingest(source);
    assert!(
        paths.iter().any(|p| p == "/feature") && paths.iter().any(|p| p == "/beta"),
        "survey keeps both arms: {paths:?}"
    );
    let messages = gap_messages(&gaps);
    assert!(
        !messages.iter().any(|m| m.contains("conditional")),
        "walked conditionals are not gaps: {messages:?}"
    );
}

/// A `case`/`when` block and a `case`/`in` pattern match are distinct
/// node kinds (`CaseNode` vs `CaseMatchNode`); both ledger as a
/// conditional `case` block.
#[test]
fn a_case_block_and_a_case_match_block_are_each_ledgered() {
    let source = b"Rails.application.routes.draw do\n  root \"pages#home\"\n  case ENV[\"MODE\"]\n  when \"admin\"\n    get \"/admin\", to: \"admin#index\"\n  end\n  case ENV[\"TIER\"]\n  in \"beta\"\n    get \"/beta\", to: \"beta#show\"\n  end\nend\n";
    assert!(
        ingest(source).is_err(),
        "strict ingest fails loud on the first `case`"
    );

    let (paths, gaps) = survey_ingest(source);
    assert!(
        paths.iter().any(|p| p == "/"),
        "the sibling `root` route survives: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p == "/admin" || p == "/beta"),
        "the predicates are not evaluated: {paths:?}"
    );
    let messages = gap_messages(&gaps);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("conditional `case` block") && m.contains("line 3")),
        "the `case`/`when` block is ledgered with its line: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("conditional `case` block") && m.contains("line 7")),
        "the `case`/`in` block is ledgered with its line: {messages:?}"
    );
}
