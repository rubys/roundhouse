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
//! `actionview` 8.1.3 (the campfire oracle's bundle), the port agrees
//! with the gem's DEFAULT — body-sanitize pass included, now that the
//! shared `sanitize` is an engine and `auto_link` runs it as Rails
//! does — on 30 of 31 probes byte for byte. The one difference is an
//! unterminated tag, which the gem's HTML5 parser closes and the
//! scanner drops: malformed markup, the scanner's stated boundary. The
//! driver pins that one with the gem's answer beside it.
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
