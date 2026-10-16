//! The ERB contract on the Ruby lane; the native twin is
//! `spinel_toolchain/erb_template.rs`.

use super::erb_template_contract as contract;

/// `ERB.new(template, trim_mode: "-").result_with_hash(hash)` and
/// `ERB::Util.html_escape` answer what CRuby's ERB answers.
#[test]
fn erb_result_with_hash_renders_after_emission() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("erb contract passed"), "{}", run.stdout);
}
