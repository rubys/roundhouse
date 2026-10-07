//! Compile the real Spinel cache; each case runs in a fresh process.
//! SPINEL=/path/to/spinel cargo test --test spinel_stmt_cache_lru -- --ignored
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command) {
    let out = command.output().expect("run cache probe");
    assert!(
        out.status.success(),
        "{command:?}: {}\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    print!("{}", String::from_utf8_lossy(&out.stdout));
}

fn probe(case: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-stmt-lru-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["db.rb", "active_support_time_parsing.rb"] {
        std::fs::copy(root.join("runtime/spinel").join(file), dir.join(file)).unwrap();
    }
    std::fs::copy(
        root.join("tests/spinel_stmt_cache_lru.rb"),
        dir.join("probe.rb"),
    )
    .unwrap();
    run(
        Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
            .args(["probe.rb", "-o", "probe"])
            .current_dir(&dir),
    );
    run(Command::new(dir.join("probe"))
        .arg(case)
        .env("DATABASE_POOL_SIZE", "1")
        .current_dir(&dir));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires Spinel and SQLite"]
fn hits_refresh_recency() {
    probe("recency");
}

#[test]
#[ignore = "requires Spinel and SQLite"]
fn eviction_follows_access_order() {
    probe("order");
}

#[test]
#[ignore = "requires Spinel and SQLite"]
fn promotion_preserves_live_cursors_until_the_lease_ends() {
    probe("live");
}

#[test]
#[ignore = "requires Spinel and SQLite"]
fn a_miss_does_not_scan_the_cache() {
    probe("misses");
}
