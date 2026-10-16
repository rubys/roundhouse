//! IO and process constants on the CRuby overlay; the native twin is
//! `spinel_toolchain/io_process_constants.rs`.

use super::io_process_constants_contract as contract;

/// `Errno::*`, `EOFError`, `File::NULL`, `Encoding::UTF_8`, `Shellwords`
/// and `PTY` resolve in app source and run as CRuby runs them.
#[test]
fn io_and_process_constants_run_as_ruby_runs_them() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
