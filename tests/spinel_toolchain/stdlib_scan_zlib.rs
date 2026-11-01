//! The native half of `emit_and_run::stdlib_scan_zlib`.

use super::stdlib_scan_zlib_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn string_scanner_math_pi_and_zlib_run_natively() {
    let run = contract::app().run_spinel_slim(&[], contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("stdlib passed"), "{}", run.stdout);
}
