//! The SHARED `auto_link` — the one every target compiles.
//!
//! `tests/overlay_sanitize_autolink.rs` covers the CRuby overlay, which
//! REDEFINES `auto_link` on top of the real `rails_autolink` chain. So
//! that file, which loads both, never executes this implementation —
//! and this implementation is the only one the strict targets have.
//! campfire runs every message body through it.
//!
//! The port is the gem's rule table, hand-scanned because neither of
//! the gem's regexes is a portable subset (`\p{Word}` is rejected at
//! compile time by matz/spinel#4143) and because it drives them with
//! `$&` / `$'` / `` $` ``, which no strict target models.
//!
//! MEASURED, not reasoned: against `rails_autolink` 1.1.8 on
//! `actionview` 8.1.3, the port agrees with `auto_link(..., :sanitize
//! => false)` on 36 of 36 probes — that setting being the gem MINUS its
//! body-sanitize pass, which is the one thing the port does not do. On
//! the gem's default it is 30 of 36, and all six differences are that
//! pass: escaped angle brackets, a dropped unknown tag, renormalised
//! attribute quotes. Not one of them is a different LINKING decision.
//! The divergence and its consequence are ledgered in
//! docs/pipeline/runtime.md.
//!
//! `ruby tests/shared_autolink.rb .` reproduces it by hand — no emit,
//! no server, and no gem needed on the runner.

use std::path::Path;
use std::process::Command;

#[test]
fn the_shared_autolink_is_the_gems_linker() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/shared_autolink.rb");
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
        stdout.contains("shared auto_link, no overlay"),
        "driver produced no banner — it failed before asserting anything\n\
         === stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        stdout.contains("ALL OK"),
        "the shared auto_link diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
