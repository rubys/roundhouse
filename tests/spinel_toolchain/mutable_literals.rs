//! The native half of `emit_and_run::mutable_literals`.

use super::mutable_literals_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_literal_appended_to_in_a_file_without_the_magic_comment_runs_natively() {
    let run = contract::app().run_spinel_slim(&[], contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("mutable literals passed"), "{}", run.stdout);
}
