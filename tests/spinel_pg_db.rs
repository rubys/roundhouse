//! The Spinel `Db` shim over spinel-pg (runtime/spinel/db_pg.rb).
//!
//! The gate compiles tests/spinel_pg_db_cases.rb next to the shim with
//! Spinel, seeded with the shim's RBS, and runs it against a live
//! PostgreSQL: bound SELECT/INSERT/UPDATE, NULL and boolean conversion,
//! row counts, handle and lease lifecycle. Nothing selects the shim for
//! an emitted app yet, so the test drives the `Db` surface directly, as
//! tests/param_binds.rs does for runtime/spinel/db.rb.
//!
//!   SPINEL_PG_DIR=/path/to/spinel-pg \
//!   DATABASE_URL=postgres://postgres:secret@127.0.0.1:5432/postgres \
//!   cargo test --test spinel_pg_db -- --ignored --nocapture
//!
//! Under `--ignored` a missing SPINEL_PG_DIR or DATABASE_URL fails the
//! test rather than skipping it. The cases create one schema named after
//! the process and drop it when they finish.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn required_env(name: &str, what: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => panic!("{name} must name {what}; this gate fails rather than skips without it"),
    }
}

fn run(command: &mut Command) -> String {
    let out = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{command:?}: {}\n{}\n{}",
        out.status,
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    print!("{stdout}");
    stdout
}

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires Spinel, SPINEL_PG_DIR (a spinel-pg checkout) and DATABASE_URL (PostgreSQL)"]
fn spinel_gate_pg_db_bound_reads_and_writes() {
    let pg_dir = required_env("SPINEL_PG_DIR", "a spinel-pg checkout");
    let url = required_env("DATABASE_URL", "a PostgreSQL server");
    assert!(
        Path::new(&pg_dir).join("pg.rb").exists(),
        "SPINEL_PG_DIR={pg_dir} has no pg.rb"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-spinel-pg-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sig")).unwrap();
    let _cleanup = ScratchDir(dir.clone());
    for name in ["db_pg.rb", "pg_errors.rb", "active_support_time_parsing.rb"] {
        let src = root.join("runtime/spinel").join(name);
        std::fs::copy(&src, dir.join(name))
            .unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    }
    for rbs in [
        "runtime/spinel/db_pg.rbs",
        "runtime/spinel/pg_errors.rbs",
        "runtime/ruby/db.rbs",
        "runtime/spinel/active_support_time_parsing.rbs",
    ] {
        let name = Path::new(rbs).file_name().unwrap();
        std::fs::copy(root.join(rbs), dir.join("sig").join(name))
            .unwrap_or_else(|e| panic!("copy {rbs}: {e}"));
    }
    std::fs::copy(
        root.join("tests/spinel_pg_db_cases.rb"),
        dir.join("main.rb"),
    )
    .unwrap();
    println!("gate tree: {}", dir.display());
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    run(Command::new(compiler)
        .args([
            "-I",
            &pg_dir,
            "--require-gate",
            "--rbs",
            ".",
            "main.rb",
            "-o",
            "pg_db_gate",
        ])
        .current_dir(&dir));
    // Bounded: a lock left behind by a broken case must fail the gate,
    // not hang the CI job.
    let stdout = run(Command::new("timeout")
        .arg("300")
        .arg(dir.join("pg_db_gate"))
        .env("DATABASE_URL", &url)
        .env(
            "SPINEL_PG_SCHEMA",
            format!("rh_spinel_pg_db_{}", std::process::id()),
        )
        .env_remove("DATABASE_POOL_SIZE")
        .env_remove("RH_SQL_TRACE")
        .current_dir(&dir));
    assert!(
        stdout.contains("spinel_pg_db: ") && stdout.contains(" checks passed"),
        "{stdout}"
    );
}

/// `(owner, name)` for every `def` in a Ruby or RBS source, with `self.`
/// kept on singleton methods. Owners are the top-level `class`/`module`
/// lines, which is all these two files use.
fn definitions(src: &str, rbs: bool) -> BTreeSet<(String, String)> {
    let mut owner = String::new();
    let mut out = BTreeSet::new();
    for line in src.lines() {
        for keyword in ["class ", "module "] {
            if let Some(rest) = line.strip_prefix(keyword) {
                owner = rest.split_whitespace().next().unwrap_or("").to_string();
            }
        }
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("def ") else {
            continue;
        };
        let end = rest
            .find(|c: char| {
                if rbs {
                    c == ':'
                } else {
                    c == '(' || c.is_whitespace()
                }
            })
            .unwrap_or(rest.len());
        out.insert((owner.clone(), rest[..end].trim().to_string()));
    }
    out
}

/// Every method the shim (db_pg.rb, pg_errors.rb) defines carries a
/// signature: the contract in runtime/ruby/db.rbs, or the shim's own
/// db_pg.rbs / pg_errors.rbs. All of them parse as RBS.
#[test]
fn db_pg_rbs_declares_every_method() {
    let read =
        |path: &str| std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let contract = read("runtime/ruby/db.rbs");
    let mut ruby = String::new();
    let mut own = String::new();
    for stem in ["runtime/spinel/db_pg", "runtime/spinel/pg_errors"] {
        ruby.push_str(&read(&format!("{stem}.rb")));
        ruby.push('\n');
        let rbs = read(&format!("{stem}.rbs"));
        roundhouse::rbs::parse_app_signatures(&rbs)
            .unwrap_or_else(|e| panic!("{stem}.rbs does not parse: {e}"));
        own.push_str(&rbs);
        own.push('\n');
    }
    roundhouse::rbs::parse_app_signatures(&contract)
        .unwrap_or_else(|e| panic!("runtime/ruby/db.rbs does not parse: {e}"));
    let mut declared = definitions(&own, true);
    declared.extend(definitions(&contract, true));
    let defined = definitions(&ruby, false);
    assert!(
        defined.len() > 60,
        "found only {} definitions",
        defined.len()
    );
    let missing: Vec<String> = defined
        .difference(&declared)
        .map(|(owner, name)| format!("{owner}#{name}"))
        .collect();
    assert!(
        missing.is_empty(),
        "the PostgreSQL shim defines methods with no RBS signature:\n  {}",
        missing.join("\n  ")
    );
    let stale: Vec<String> = definitions(&own, true)
        .difference(&defined)
        .map(|(owner, name)| format!("{owner}#{name}"))
        .collect();
    assert!(
        stale.is_empty(),
        "the PostgreSQL shim RBS declares methods the shim does not define:\n  {}",
        stale.join("\n  ")
    );
}
