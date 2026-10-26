//! A request that raises gives its database connection back, on the
//! spinel lane.
//!
//! `Db.with_connection` (`runtime/spinel/db.rb`) released its lease on
//! the happy path only, so every 500 leaked one connection for good.
//! After a pool's worth on one shard, the next lease there parked
//! forever and the binary stopped answering every route that shard's
//! threads served. `scripts/campfire-http-shape` found it on campfire:
//! a sweep that alternates routes kept landing the one route that raised
//! on the same shard, which went dark after four.
//!
//! The driver (`tests/spinel_db_lease.rb`) is compiled BY SPINEL: db.rb's
//! sqlite3 calls are `ffi_func`s with no CRuby spelling.
//!
//! Marked `#[ignore]` — needs `spinel` on PATH and sqlite3 where the C
//! compiler finds it. Invoke:
//!
//!     PATH=$HOME/git/spinel/bin:$PATH cargo test --test spinel_db_lease -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_dir() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-spinel-db-lease")
}

#[test]
#[ignore]
fn a_request_that_raises_releases_its_connection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir();
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/db.rb", "db.rb"),
        ("runtime/spinel/active_support_time_parsing.rb", "active_support_time_parsing.rb"),
        ("runtime/spinel/active_support_interval.rb", "active_support_interval.rb"),
        ("runtime/spinel/active_support_duration.rb", "active_support_duration.rb"),
        ("tests/spinel_db_lease.rb", "driver.rb"),
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

    let run = Command::new(scratch.join("driver"))
        .arg(scratch.join("lease-probe.sqlite3"))
        .output()
        .expect("run driver");
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
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("ok ")).count(),
        4,
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}
