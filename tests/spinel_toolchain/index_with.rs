//! `index_with` compiled natively; the Ruby-lane twin is
//! `emit_and_run/index_with.rs`.

use super::index_with_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn index_with_answers_rails_hash_natively() {
    let run = contract::overlay().run_spinel(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("index_with contract passed"), "{}", run.stdout);
}
