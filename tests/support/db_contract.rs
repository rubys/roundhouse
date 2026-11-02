//! The Db contract suite's lanes: which shim, on which runtime, against
//! which database, and the runner that copies one lane into a scratch
//! directory and runs it.
//!
//! The suite itself is Ruby, in tests/db_contract/:
//!
//!   contract_cases.rb  — the Db contract (runtime/ruby/db.rbs), every lane
//!   postgres_cases.rb  — what any PostgreSQL Db adds, PostgreSQL lanes
//!   <shim>_lane.rb     — a PostgreSQL lane's hook and its shim's own cases
//!   sqlite_main.rb / postgres_main.rb — the programs a lane runs
//!
//! A lane copies its shim in as `db.rb`, the runtime's prelude
//! (`cruby_prelude.rb` or `spinel_prelude.rb`) as `prelude.rb`, and the
//! lane file as `lane.rb`, then runs the backend's main file: under
//! `ruby` for CRuby, compiled by Spinel for Spinel. Each suite prints
//! `<suite>: N checks passed`, and every lane must report exactly the
//! counts below, so a lane that skipped a case fails rather than passes.
//!
//! Pointing a new shim at the suite is one row in `LANES`. The tests that
//! run lanes select them by runtime and backend, so the row is picked up
//! by tests/db_contract.rs (CRuby) or tests/spinel_db_contract.rs and
//! tests/spinel_pg_db.rs (Spinel). A PostgreSQL row names a lane file;
//! db_pg_lane.rb shows the one hook it must define.
#![allow(dead_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "pg_test_url.rs"]
pub mod pg_test_url;

/// Checks in contract_cases.rb; every lane runs all of them.
pub const CONTRACT_CHECKS: usize = 129;
/// Checks in postgres_cases.rb; every PostgreSQL lane runs all of them.
pub const POSTGRES_CHECKS: usize = 45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    Cruby,
    Spinel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

pub struct Lane {
    /// Names the lane in output and scratch paths.
    pub name: &'static str,
    /// The `Db` shim, copied in as db.rb.
    pub shim: &'static str,
    pub runtime: Runtime,
    pub backend: Backend,
    /// Runtime files the shim requires, copied beside it by basename.
    pub requires: &'static [&'static str],
    /// Runtime files copied beside it under another name: a runtime's own
    /// variant of a file the shim requires (`pg_driver_cruby.rb` as
    /// `pg_driver.rb`), as the emitted tree ships it.
    pub requires_as: &'static [(&'static str, &'static str)],
    /// RBS handed to Spinel with `--rbs`, copied into sig/.
    pub rbs: &'static [&'static str],
    /// A directory the compile adds with `-I`, named by this variable
    /// (a Spinel package checkout). Required when set.
    pub include_env: Option<(&'static str, &'static str)>,
    /// PostgreSQL lanes: the lane file (hook and shim-own cases), and
    /// the checks it reports under its `lane_name`, the shim's file stem.
    pub lane_file: Option<&'static str>,
    pub lane_checks: usize,
}

const SPINEL_TIME: &[&str] = &[
    "runtime/spinel/active_support_time_parsing.rb",
    "runtime/spinel/active_support_interval.rb",
    "runtime/spinel/active_support_duration.rb",
];

pub const LANES: &[Lane] = &[
    Lane {
        name: "db_cruby",
        shim: "runtime/spinel/db_cruby.rb",
        runtime: Runtime::Cruby,
        backend: Backend::Sqlite,
        requires: &[],
        requires_as: &[],
        rbs: &[],
        include_env: None,
        lane_file: None,
        lane_checks: 0,
    },
    Lane {
        name: "db",
        shim: "runtime/spinel/db.rb",
        runtime: Runtime::Spinel,
        backend: Backend::Sqlite,
        requires: SPINEL_TIME,
        requires_as: &[],
        rbs: &[],
        include_env: None,
        lane_file: None,
        lane_checks: 0,
    },
    Lane {
        name: "db_pg",
        shim: "runtime/spinel/db_pg.rb",
        runtime: Runtime::Spinel,
        backend: Backend::Postgres,
        requires: &[
            "runtime/spinel/pg_errors.rb",
            "runtime/spinel/pg_driver.rb",
            "runtime/spinel/active_support_time_parsing.rb",
            "runtime/spinel/active_support_interval.rb",
            "runtime/spinel/active_support_duration.rb",
        ],
        requires_as: &[],
        rbs: &[
            "runtime/spinel/db_pg.rbs",
            "runtime/spinel/pg_driver.rbs",
            "runtime/spinel/pg_errors.rbs",
            "runtime/ruby/db.rbs",
            "runtime/spinel/active_support_time_parsing.rbs",
            "runtime/spinel/active_support_interval.rbs",
            "runtime/spinel/active_support_duration.rbs",
        ],
        include_env: Some(("SPINEL_PG_DIR", "a spinel-pg checkout")),
        lane_file: Some("tests/db_contract/db_pg_lane.rb"),
        lane_checks: 118,
    },
    Lane {
        name: "db_pg_cruby",
        shim: "runtime/spinel/db_pg.rb",
        runtime: Runtime::Cruby,
        backend: Backend::Postgres,
        requires: &[
            "runtime/spinel/pg_errors.rb",
            "runtime/spinel/active_support_time_parsing.rb",
            "runtime/spinel/active_support_interval.rb",
            "runtime/spinel/active_support_duration.rb",
        ],
        requires_as: &[("runtime/spinel/pg_driver_cruby.rb", "pg_driver.rb")],
        rbs: &[],
        include_env: None,
        lane_file: Some("tests/db_contract/db_pg_lane.rb"),
        lane_checks: 118,
    },
];

/// The lanes one test runs.
pub fn lanes(runtime: Runtime, backend: Backend) -> impl Iterator<Item = &'static Lane> {
    LANES
        .iter()
        .filter(move |l| l.runtime == runtime && l.backend == backend)
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::copy(from, to)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
}

fn run(command: &mut Command) -> String {
    let out = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{command:?}: {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    print!("{stdout}");
    stdout
}

/// A lane's program may run this long. A lease that leaks, or a lock a
/// broken case leaves behind, must fail the lane rather than hang the job.
pub const LANE_DEADLINE: Duration = Duration::from_secs(300);

/// `run`, killed at `deadline`. Enforced here rather than by timeout(1),
/// which not every host has under that name. `missing` explains a
/// program that cannot be started.
pub fn run_bounded(command: &mut Command, deadline: Duration, missing: &str) -> String {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{command:?}: {e}; {missing}"));
    // Drained while the child runs, so a full pipe never stalls it.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            String::from_utf8_lossy(&out).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the lane") {
            break Some(status);
        }
        if start.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = stdout.join().unwrap();
    let stderr = stderr.join().unwrap();
    let Some(status) = status else {
        panic!(
            "{command:?}: still running after {}s, killed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
            deadline.as_secs()
        );
    };
    assert!(
        status.success(),
        "{command:?}: {status}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    print!("{stdout}");
    stdout
}

/// The count a suite reported, or a failure naming the missing line.
fn reported(stdout: &str, suite: &str) -> usize {
    let prefix = format!("{suite}: ");
    stdout
        .lines()
        .find_map(|l| {
            l.strip_prefix(&prefix)?
                .strip_suffix(" checks passed")?
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no `{suite}: N checks passed` line\n{stdout}"))
}

/// Runs one lane. A PostgreSQL lane returns without running (after
/// saying why) when ROUNDHOUSE_PG_TEST_URL is unset; see pg_test_url.rs.
pub fn run_lane(lane: &Lane) {
    let test = format!("db_contract_{}", lane.name);
    let pg = match lane.backend {
        Backend::Sqlite => None,
        Backend::Postgres => match pg_test_url::pg_test_db(&test) {
            Some(db) => Some(db),
            None => return,
        },
    };
    let include = lane
        .include_env
        .map(|(var, what)| match std::env::var(var) {
            Ok(v) if !v.is_empty() => v,
            _ => panic!("{var} must name {what} for the {} lane", lane.name),
        });
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sig")).unwrap();
    let _cleanup = ScratchDir(dir.clone());

    let suite = root().join("tests/db_contract");
    copy(&root().join(lane.shim), &dir.join("db.rb"));
    for file in lane.requires {
        let path = Path::new(file);
        copy(&root().join(path), &dir.join(path.file_name().unwrap()));
    }
    for (file, name) in lane.requires_as {
        copy(&root().join(file), &dir.join(name));
    }
    for file in lane.rbs {
        let path = Path::new(file);
        copy(
            &root().join(path),
            &dir.join("sig").join(path.file_name().unwrap()),
        );
    }
    let prelude = match lane.runtime {
        Runtime::Cruby => "cruby_prelude.rb",
        Runtime::Spinel => "spinel_prelude.rb",
    };
    copy(&suite.join(prelude), &dir.join("prelude.rb"));
    for file in ["support.rb", "contract_cases.rb", "postgres_cases.rb"] {
        copy(&suite.join(file), &dir.join(file));
    }
    let main = match lane.backend {
        Backend::Sqlite => "sqlite_main.rb",
        Backend::Postgres => {
            let lane_file = lane
                .lane_file
                .unwrap_or_else(|| panic!("the PostgreSQL lane {} names no lane file", lane.name));
            copy(&root().join(lane_file), &dir.join("lane.rb"));
            "postgres_main.rb"
        }
    };
    copy(&suite.join(main), &dir.join("main.rb"));
    println!("{} lane tree: {}", lane.name, dir.display());

    let (mut program, missing) = match lane.runtime {
        Runtime::Cruby => {
            let mut program = Command::new("ruby");
            program.arg("main.rb").env("ROUNDHOUSE_ROOT", root());
            (
                program,
                "the CRuby lanes need `ruby` (with the sqlite3 gem for SQLite) on PATH",
            )
        }
        Runtime::Spinel => {
            if std::env::var_os("RH_SPINEL_BUNDLE_COLLECT").is_some() {
                panic!("spinel bundle collect: not bundled");
            }
            let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
            let mut compile = Command::new(compiler);
            if let Some(include) = &include {
                compile.args(["-I", include, "--strict-require"]);
            }
            if !lane.rbs.is_empty() {
                compile.args(["--rbs", "."]);
            }
            run(compile
                .args(["main.rb", "-o", "contract_gate"])
                .current_dir(&dir));
            (
                Command::new(dir.join("contract_gate")),
                "Spinel did not leave the compiled lane behind",
            )
        }
    };
    if lane.backend == Backend::Sqlite {
        program.arg(dir.join("contract.sqlite3"));
    }
    if let Some(db) = &pg {
        program
            .env("ROUNDHOUSE_PG_TEST_URL", &db.url)
            .env("ROUNDHOUSE_PG_TEST_SCHEMA", &db.schema);
    }
    let stdout = run_bounded(
        program
            .env_remove("DATABASE_POOL_SIZE")
            .env_remove("RH_SQL_TRACE")
            .current_dir(&dir),
        LANE_DEADLINE,
        missing,
    );

    assert_eq!(
        reported(&stdout, "contract"),
        CONTRACT_CHECKS,
        "the {} lane ran a different number of contract checks than contract_cases.rb makes",
        lane.name
    );
    if lane.backend == Backend::Postgres {
        assert_eq!(
            reported(&stdout, "postgres"),
            POSTGRES_CHECKS,
            "the {} lane ran a different number of PostgreSQL checks than postgres_cases.rb makes",
            lane.name
        );
        // The lane file reports under its shim's name, so one shim's
        // lanes on both runtimes share it.
        let shim = Path::new(lane.shim).file_stem().and_then(|stem| stem.to_str()).unwrap();
        assert_eq!(
            reported(&stdout, shim),
            lane.lane_checks,
            "the {} lane's own checks",
            lane.name
        );
    }
}
