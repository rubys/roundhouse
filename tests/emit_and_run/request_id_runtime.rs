//! X-Request-Id and X-Runtime on the Ruby lane; the native twin is
//! `spinel_toolchain/request_id_runtime.rs`.

use super::request_id_runtime_contract as contract;

/// Every app response carries a request id (the client's, cleaned, or
/// a fresh UUID) and the time the app took, a 404 included.
#[test]
fn responses_carry_request_id_and_runtime() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("request_id_runtime contract passed"), "{}", run.stdout);
}
