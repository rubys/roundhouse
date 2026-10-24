//! `SQLite3::Database` batch, transaction and Float binds compiled
//! natively (`runtime/spinel/sqlite3_database.rb`), answering what the
//! sqlite3 gem answers; the shared contract is
//! `tests/support/sqlite3_batch_transaction.rs`.

use super::sqlite3_batch_transaction_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn sqlite3_batch_transaction_and_float_binds_run_natively() {
    let run = contract::overlay().run_spinel(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
