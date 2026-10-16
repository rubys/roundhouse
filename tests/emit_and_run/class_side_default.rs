//! Class-side parameter defaults on the CRuby overlay; the native twin is
//! `spinel_toolchain/class_side_default.rs`.

use super::class_side_default_contract as contract;

/// A class method's default calls the class's own sibling method.
#[test]
fn a_class_methods_default_reads_a_class_method() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
