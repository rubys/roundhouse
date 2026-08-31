//! The SHARED safe-list sanitizer — the one every target compiles.
//!
//! The CRuby overlay redefines `sanitize` / `sanitize_allowing` on top
//! of the real `rails-html-sanitizer` gem, so the overlay lane never
//! executes this implementation — and this implementation is the only
//! one the strict targets have. campfire runs EVERY message body
//! through it: Trix always submits HTML, and until this port existed
//! the shared entry points raised on any input containing markup,
//! which campfire's `rescue Exception` rendered as an empty message.
//!
//! MEASURED, not reasoned: against `Rails::HTML5::SafeListSanitizer`
//! (rails-html-sanitizer 1.7.1 / loofah 2.25.2, campfire's lock) the
//! port agrees byte for byte on 75 of 81 corpus probes; the six
//! differences are the declared policies — entities kept as written
//! rather than decoded (text and kept attribute values), source tag
//! order kept rather than HTML5 tree reconstruction on malformed
//! nesting, and a C1 numeric reference in a URL blocked where the
//! gem's parser remaps it to its Windows-1252 character first. Every
//! one is more-escaped or more-blocked, never less. Ledgered in
//! docs/pipeline/runtime.md.
//!
//! `ruby tests/shared_sanitize.rb .` reproduces it by hand — no emit,
//! no server, and no gem needed on the runner. The strict-lane
//! EXECUTION gate is `view_helpers_ext_test_passes_spinel` in
//! tests/framework_tests_spinel.rs, which compiles the same file under
//! spinel and runs a per-code-path probe set.

use std::path::Path;
use std::process::Command;

#[test]
fn the_shared_sanitizer_is_the_gems_safe_list() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/shared_sanitize.rb");
    let out = Command::new("ruby")
        .arg(&driver)
        .arg(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The banner is printed before the first assertion; without it the
    // driver died on load, which `status.success()` alone would report
    // as indistinguishable from a wrong value.
    assert!(
        stdout.contains("shared sanitize, no overlay"),
        "driver produced no banner — it failed before asserting anything\n\
         === stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        stdout.contains("ALL OK"),
        "the shared sanitizer diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
