//! IO and process constants compiled natively: Spinel's runtime and its
//! `pty` package, plus the module-only `shellwords` / `IO::NULL` ports.
//! Block-form `PTY.spawn` is a Spinel subset gap (`NotImplementedError`);
//! the CRuby overlay twin pins those shapes.

use super::io_process_constants_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn io_and_process_constants_run_as_ruby_runs_them_natively() {
    let run = contract::spinel_overlay().run_spinel(contract::SPINEL_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::SPINEL_EXPECTED, "stderr:\n{}", run.stderr);
}
