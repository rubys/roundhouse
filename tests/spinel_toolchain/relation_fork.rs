//! Relations derived from one base, compiled natively and served over
//! HTTP (the model needs the database); the CRuby twin is
//! `emit_and_run/relation_fork.rs`.

use super::{native_http, relation_fork_contract as contract};
use roundhouse::project::BuildTarget;

/// Sibling relations compile and answer as on CRuby.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn query_methods_leave_the_receiver_unchanged_natively() {
    let (tree, errors) = contract::served_overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let report = server.get("/relation-fork-report");
    assert_eq!(report.status, 200, "{}", server.log());
    assert_eq!(report.body, contract::EXPECTED, "{}", server.log());
}
