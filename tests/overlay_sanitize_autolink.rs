//! The CRuby overlay's escape surface, exercised.
//!
//! `sanitize` / `strip_tags` / `auto_link` live in
//! `ruby_overlay/runtime/action_view_sanitize.rb`, and NOTHING ELSE
//! covers them. campfire's own suite passes with
//! `rails-html-sanitizer` hidden behind a LoadError shim — no test in
//! it asserts message-body markup — and the only other check is the
//! campfire oracle comparison, which does not run in CI. That gap was
//! opened by the commit that added these helpers; this closes it.
//!
//! The driver loads the overlay files directly, in boot.rb's order, so
//! there is no emit and no server: `ruby tests/overlay_sanitize_autolink.rb .`
//! reproduces it by hand.
//!
//! Every expected value was MEASURED against the real
//! `Rails::HTML5::SafeListSanitizer` / `Rails::HTML5::FullSanitizer` or
//! against the `rails_autolink` gem, not reasoned out. The `auto_link`
//! port agrees with that gem on 13 of 14 probes; the fourteenth is an
//! email address, where the anchor goes through our `mail_to` and comes
//! back with `href` ahead of the caller's attributes instead of behind
//! them — a general divergence, ledgered in docs/pipeline/runtime.md,
//! not an auto_link bug.

use std::path::Path;
use std::process::Command;

#[test]
fn the_overlay_escape_surface_matches_rails() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/overlay_sanitize_autolink.rb");
    let out = Command::new("ruby")
        .arg(&driver)
        .arg(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // A vendor line is always printed; its absence means the driver
    // died before the first assertion, which `status.success()` alone
    // would report as the same failure as a wrong value.
    assert!(
        stdout.contains("vendor "),
        "driver produced no vendor line — it failed before asserting anything\n\
         === stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        stdout.contains("ALL OK"),
        "overlay escape surface diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
