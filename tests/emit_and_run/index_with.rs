//! `index_with` on the Ruby lane; the native twin is
//! `spinel_toolchain/index_with.rs`.

use super::index_with_contract as contract;

/// `list.index_with(value)` and `list.index_with { |x| ... }` (and `&:sym`)
/// answer Rails' Hash, in a class-body constant and in a method body.
#[test]
fn index_with_answers_rails_hash() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("index_with contract passed"), "{}", run.stdout);
}
