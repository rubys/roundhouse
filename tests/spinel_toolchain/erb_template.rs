//! The ERB contract compiled natively; the Ruby-lane twin is
//! `emit_and_run/erb_template.rs`.

use super::erb_template_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn erb_result_with_hash_renders_natively() {
    let run = contract::overlay().run_spinel(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("erb contract passed"), "{}", run.stdout);
}
