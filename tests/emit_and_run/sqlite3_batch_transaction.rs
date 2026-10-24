//! `SQLite3::Database` batch, transaction and Float binds on the CRuby
//! overlay, i.e. the sqlite3 gem's own answers; the shared contract is
//! `tests/support/sqlite3_batch_transaction.rs`.

use super::sqlite3_batch_transaction_contract as contract;

#[test]
fn sqlite3_batch_transaction_and_float_binds_run() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
