//! `5.megabytes` → `5 * 1048576` (`lower::byte_size`), and the
//! class-body CONSTANT reach that makes it matter.
//!
//! ActiveSupport's byte helpers are defined as plain multiplication —
//! unlike the sibling duration helpers, there is no value class behind
//! them — so the grounded form is the arithmetic itself and no target
//! needs runtime support.
//!
//! The reach is the other half. Where the corpus writes these is class
//! bodies (`Opengraph::Fetch::MAX_BODY_SIZE = 5.megabytes`,
//! `Membership::Connectable::CONNECTION_TTL = 60.seconds`), and
//! `for_each_hook_body` — the one definition of what the post-analyze
//! passes may rewrite — used to skip constant initializers. An
//! ungrounded send there is worse than one in a method: it runs at
//! `require`, so it kills the process at load rather than at first call.

use roundhouse::lower::byte_size::apply_byte_size_lowering;
use roundhouse::lower::duration::apply_duration_lowering;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::App;

fn lower(source: &str) -> String {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    apply_byte_size_lowering(&mut app);
    apply_duration_lowering(&mut app);
    format!("{:?}", app.library_classes)
}

fn method_body(expr: &str) -> String {
    lower(&format!("class Thing\n  def q\n    {expr}\n  end\nend\n"))
}

#[test]
fn megabytes_folds_to_the_binary_factor() {
    let out = method_body("5.megabytes");
    assert!(out.contains("1048576"), "expected 1024^2:\n{out}");
    assert!(!out.contains("megabytes"), "the helper should be gone:\n{out}");
}

#[test]
fn every_unit_is_binary_not_si() {
    // ActiveSupport is 1024-based; 1000-based factors would be silently
    // wrong for anything comparing against a real file size.
    for (src, factor) in [
        ("2.kilobytes", "1024"),
        ("3.gigabytes", "1073741824"),
        ("1.terabytes", "1099511627776"),
    ] {
        let out = method_body(src);
        assert!(out.contains(factor), "{src} should fold to {factor}:\n{out}");
    }
}

#[test]
fn single_byte_is_the_identity_not_a_multiplication() {
    let out = method_body("n = 4\n    n.bytes");
    assert!(!out.contains("\"*\""), "`bytes` is self, not `self * 1`:\n{out}");
}

#[test]
fn string_receiver_is_left_alone() {
    // `String#bytes` answers an Array of byte values — a different
    // method entirely. A wrong fold here turns a byte array into a
    // number, so an unprovable receiver must decline.
    let out = method_body(r#""abc".bytes"#);
    assert!(out.contains("bytes"), "String#bytes must survive:\n{out}");
}

#[test]
fn class_body_constant_initializer_is_reached() {
    // The reason the pass exists at all: this is load-time code, and
    // `for_each_hook_body` skipped it until byte_size needed it.
    let out = lower("class Fetch\n  MAX_BODY_SIZE = 5.megabytes\nend\n");
    assert!(out.contains("1048576"), "constant initializer not lowered:\n{out}");
}

#[test]
fn class_body_constant_reaches_the_duration_pass_too() {
    // Same hook, the sibling pass — campfire's
    // `Membership::Connectable::CONNECTION_TTL = 60.seconds`, which
    // raised NoMethodError at `require` before the hook was widened.
    let out = lower("class Connectable\n  CONNECTION_TTL = 60.seconds\nend\n");
    assert!(
        out.contains("Duration"),
        "constant initializer should reach duration grounding:\n{out}"
    );
}
