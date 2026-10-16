//! `Vary: Accept` on the Ruby lane; the native twin is
//! `spinel_toolchain/vary_accept.rs`.

use super::vary_accept_contract as contract;

/// A render whose format came from the Accept header carries
/// `Vary: Accept`; a browser's Accept, a format param, a redirect and a
/// `head` do not.
#[test]
fn renders_negotiated_from_accept_carry_vary_accept() {
    let run = contract::overlay().run_ruby(&contract::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("vary_accept contract passed"), "{}", run.stdout);
}
