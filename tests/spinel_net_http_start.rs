//! The reopened `Net::HTTP#start` (`runtime/spinel/net_http.rb`) keeps
//! CRuby's block form, compiled and run.
//!
//! The reopen makes `#start` lazy so the stub table answers before any
//! socket opens. It used to drop its block, which was harmless while the
//! package's `Net::HTTP.start` connected and then yielded the session
//! itself. matz/spinel#8048 (169fa99b5) moved `Net::HTTP.start`,
//! `.get_response` and `.post_form` onto `http.start { |h| ... }`, as
//! CRuby's are written, and from then on the block never ran:
//! `Net::HTTP.start(host, port) { |http| http.request(req) }` answered the
//! session and sent nothing (campfire's `Opengraph::Fetch` then spins
//! through MAX_REDIRECTS).
//!
//! The driver (`tests/spinel_net_http_start.rb`) is compiled BY SPINEL:
//! the reopen is written against spinel's net package, which CRuby's
//! Net::HTTP does not spell, so there is no interpreted lane for it. Every
//! request it makes is answered by the stub table; no network is needed.
//!
//! Marked `#[ignore]` — needs `spinel` on PATH. Invoke:
//!
//!     PATH=$HOME/git/spinel/bin:$PATH cargo test --test spinel_net_http_start -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

const CHECKS: usize = 23;

fn scratch_dir() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-spinel-net-http-start")
}

#[test]
#[ignore]
fn the_lazy_start_yields_the_session_and_answers_the_blocks_value() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir();
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/net_http.rb", "net_http.rb"),
        ("runtime/spinel/http_stub.rb", "http_stub.rb"),
        ("runtime/spinel/tcp_socket_stub.rb", "tcp_socket_stub.rb"),
        ("tests/spinel_net_http_start.rb", "driver.rb"),
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
    // A crash (e.g. a segfault mid-run) can still leave a prefix of
    // passing `ok` lines on stdout with no `FAIL` among them; checking
    // the exit status first is what catches that before the line checks
    // below mistake a partial run for a clean one.
    assert!(
        run.status.success(),
        "driver exited with {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        run.status
    );
    // `done` is the last line; its absence means the binary died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("ok ")).count(),
        CHECKS,
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
