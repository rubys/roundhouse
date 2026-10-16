//! Rack::ETag and Rack::ConditionalGet on the Ruby lane; the native
//! twin is `spinel_toolchain/conditional_get.rs`.

use super::conditional_get_contract as contract;

/// A page gets the body ETag and Rails' default Cache-Control, a
/// matching If-None-Match a bodyless 304, a redirect `no-cache`.
#[test]
fn responses_carry_etag_cache_control_and_answer_304() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("conditional_get contract passed"), "{}", run.stdout);
}
