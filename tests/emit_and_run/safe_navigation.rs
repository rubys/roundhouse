//! Safe navigation on the CRuby overlay; the native twin is
//! `spinel_toolchain/safe_navigation.rs`.

use super::safe_navigation_contract as contract;

/// `a&.b` evaluates `a` once and guards only `nil`: a `false` receiver
/// dispatches, as in CRuby.
#[test]
fn safe_navigation_guards_only_nil() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
