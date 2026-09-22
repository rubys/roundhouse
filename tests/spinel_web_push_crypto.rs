//! The web-push gem's cryptography, ported onto spinel's openssl
//! package (`runtime/spinel/web_push_crypto.rb`), compiled and run.
//!
//! The driver (`tests/spinel_web_push_crypto.rb`) is compiled BY SPINEL:
//! the port is written against the package's byte-level key API, which
//! CRuby's openssl does not spell, so there is no interpreted lane for
//! it. Its oracle is the gem's own output for RFC 8291's inputs, pinned
//! in the driver with the recipe that produced it.
//!
//! Marked `#[ignore]` — needs `spinel` on PATH and libssl/libcrypto
//! where the C compiler finds them (on a Homebrew Mac, `CPATH` and
//! `LIBRARY_PATH` pointing into `openssl@3`). Invoke:
//!
//!     PATH=$HOME/git/spinel/bin:$PATH cargo test --test spinel_web_push_crypto -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_dir() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-spinel-web-push-crypto")
}

#[test]
#[ignore]
fn the_ported_crypto_matches_the_gem_and_opens_on_the_receivers_side() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir();
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/web_push_crypto.rb", "web_push_crypto.rb"),
        ("runtime/spinel/base64.rb", "base64.rb"),
        ("tests/spinel_web_push_crypto.rb", "driver.rb"),
    ] {
        std::fs::copy(root.join(from), scratch.join(to)).expect("copy");
    }

    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&scratch)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile the driver\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(scratch.join("driver")).output().expect("run driver");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    // `done` is the last line; its absence means the binary died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert!(
        stdout.lines().filter(|l| l.starts_with("ok ")).count() >= 10,
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}
