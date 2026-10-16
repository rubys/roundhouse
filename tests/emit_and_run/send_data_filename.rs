//! `send_data(filename:)` on the Ruby lane; the native twin is
//! `spinel_toolchain/send_data_filename.rs`.

use super::send_data_filename_contract as contract;

#[test]
fn send_data_filename_writes_rails_content_disposition() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("send_data filename passed"), "{}", run.stdout);
}
