//! Relations derived from one base, on the CRuby overlay; the native twin
//! is `spinel_toolchain/relation_fork.rs`.

use super::relation_fork_contract as contract;

/// Each query method answers a new relation and leaves the base as it
/// was, so siblings derived from one base answer as in Rails.
#[test]
fn query_methods_leave_the_receiver_unchanged() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
