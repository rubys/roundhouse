//! Exercise Cargo's actual build-script invalidation with ordinary and
//! linked Git checkouts. A missing rerun-if-changed path causes every
//! no-op build to rerun, even when the embedded commit remains correct.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    linked: PathBuf,
    counter: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run(dir: &Path, program: &str, args: &[&str], commit: Option<&str>) -> String {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args);
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "ROUNDHOUSE_COMMIT",
    ] {
        cmd.env_remove(key);
    }
    if let Some(commit) = commit {
        cmd.env("ROUNDHOUSE_COMMIT", commit);
    }
    cmd.env("CARGO_TARGET_DIR", dir.join("target"));
    let out = cmd.output().expect("run fixture command");
    assert!(
        out.status.success(),
        "{program} {args:?}:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let mut configured = vec![
        "-c",
        "user.name=Stamp Test",
        "-c",
        "user.email=stamp@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
    ];
    configured.extend_from_slice(args);
    run(dir, "git", &configured, None)
}

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "roundhouse-build-stamp-{}-{nonce}",
            std::process::id()
        ));
        let repo = root.join("repo");
        let linked = root.join("linked");
        let counter = root.join("build-runs");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"stamp_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            repo.join("src/main.rs"),
            "fn main() { println!(\"{}\", env!(\"ROUNDHOUSE_COMMIT\")); }",
        )
        .unwrap();
        // Run the production build script, counting executions outside all
        // watched directories. Cached cargo warnings cannot measure reruns.
        fs::write(repo.join("build.rs"), format!(
            "mod stamp {{\n{}\npub fn run() {{ main(); }}\n}}\n\
             fn main() {{ stamp::run(); use std::io::Write; \
             writeln!(std::fs::OpenOptions::new().create(true).append(true).open({:?}).unwrap(), \"run\").unwrap(); }}\n",
            include_str!("../build.rs"), counter.to_str().unwrap())).unwrap();
        fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        for stem in ["ruby", "spinel"] {
            let dir = repo.join("runtime").join(stem);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("probe.rb"), "# fixture\n").unwrap();
        }
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "initial"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "nested/probe",
                linked.to_str().unwrap(),
            ],
        );
        Self {
            root,
            repo,
            linked,
            counter,
        }
    }

    fn build(&self, dir: &Path, label: &str, expected_runs: usize, commit: Option<&str>) {
        let runs = || {
            fs::read_to_string(&self.counter)
                .unwrap_or_default()
                .lines()
                .count()
        };
        let before = runs();
        let stamp = run(dir, env!("CARGO"), &["run", "--quiet", "--offline"], commit);
        assert_eq!(
            stamp,
            commit
                .map(str::to_owned)
                .unwrap_or_else(|| git(dir, &["rev-parse", "--short=8", "HEAD"])),
            "{label}: embedded stamp"
        );
        assert_eq!(
            runs() - before,
            expected_runs,
            "{label}: build-script executions"
        );
    }

    fn branch_cycle(&self, dir: &Path) {
        // The other checkout's cycle may have packed this branch too.
        git(
            dir,
            &["commit", "--allow-empty", "-m", "loose starting ref"],
        );
        self.build(dir, "loose first build", 1, None);
        self.build(dir, "loose no-op", 0, None);
        git(dir, &["commit", "--allow-empty", "-m", "advance loose"]);
        self.build(dir, "loose advance", 1, None);
        self.build(dir, "loose advanced no-op", 0, None);
        git(dir, &["pack-refs", "--all", "--prune"]);
        self.build(dir, "pack loose ref", 1, None);
        self.build(dir, "packed no-op", 0, None);
        // Move HEAD but leave only a packed ref by the next Cargo invocation.
        git(
            dir,
            &["commit", "--allow-empty", "-m", "advance then repack"],
        );
        git(dir, &["pack-refs", "--all", "--prune"]);
        self.build(dir, "packed advance", 1, None);
        self.build(dir, "packed advanced no-op", 0, None);
        git(
            dir,
            &["commit", "--allow-empty", "-m", "recreate loose ref"],
        );
        self.build(dir, "packed to loose", 1, None);
        self.build(dir, "recreated loose no-op", 0, None);
    }
}

#[test]
fn commit_stamp_tracks_loose_packed_and_detached_worktree_heads() {
    let f = Fixture::new();
    f.branch_cycle(&f.repo);
    f.branch_cycle(&f.linked);
    git(&f.linked, &["checkout", "--detach"]);
    f.build(&f.linked, "detach", 1, None);
    f.build(&f.linked, "detached no-op", 0, None);
    git(
        &f.linked,
        &["commit", "--allow-empty", "-m", "advance detached"],
    );
    f.build(&f.linked, "detached advance", 1, None);
    f.build(&f.linked, "override", 1, Some("frozen123"));
    git(
        &f.linked,
        &["commit", "--allow-empty", "-m", "advance overridden"],
    );
    f.build(
        &f.linked,
        "override ignores Git changes",
        0,
        Some("frozen123"),
    );
    f.build(&f.linked, "override change", 1, Some("frozen456"));
    f.build(&f.linked, "override removed", 1, None);
    f.build(&f.linked, "override removed no-op", 0, None);
}
