//! Emit-and-run: the half of a claim that a diagnostic count cannot make.
//!
//! A change that makes an error diagnostic go away is a claim that the
//! construct is now *supported*, and supported means the emitted program
//! runs, not merely that `check` is quiet. `Model.human_attribute_name`
//! (#139) is the example that motivated this file: typing it as a String
//! took `check` to 0 errors while the emitted view raised
//! `undefined method 'human_attribute_name' for class Article`, because
//! no runtime defined it. Before the change it was an error, which this
//! project reads as "not supported yet"; after, it was silently broken.
//!
//! This harness takes the real-blog fixture, applies a few edits (an
//! overlay), and then does what a user of the output would: runs
//! `check`'s error gate, emits the Ruby target, and runs one of the
//! emitted test files with CRuby.
//!
//! ```ignore
//! #[path = "support/emit_and_run.rs"]
//! mod emit_and_run;
//!
//! emit_and_run::real_blog()
//!     .edit(
//!         "app/views/articles/_form.html.erb",
//!         "<%= form.label :title %>",
//!         "<%= Article.human_attribute_name(:title) %>",
//!     )
//!     .run_test("test/controllers/articles_controller_test.rb")
//!     .assert_passes();
//! ```
//!
//! The real-blog controller tests render every page, so an edit to a
//! view, a controller, or a model is exercised by
//! `articles_controller_test.rb` with no new test to write. When the
//! construct needs a probe of its own, `run_ruby` boots the emitted app
//! (`main.rb`, SQLite in memory) and runs a script against it.
//!
//! Requires `ruby` with the `sqlite3` gem, the same prerequisite as
//! generating the fixture. It does not skip when they are absent: a
//! harness that skips is a test that passes, and the CI unit job ran
//! `tests/relation_runtime_cruby.rs` that way, printing "skipping" into
//! captured output, until the job installed the gem.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::{Diagnostic, Severity};
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

/// Entries of the fixture that are neither read by ingest nor shipped
/// by emit: Rails' runtime scratch, and the TruffleRuby lanes' bundles
/// (440 MB between them) that the benchmark scripts leave behind.
const SKIP: &[&str] = &["tmp", "log", "storage", "node_modules", ".git"];

/// Start from `fixtures/real-blog`.
pub fn real_blog() -> Overlay {
    Overlay { base: roundhouse::fixtures::real_blog().to_path_buf(), scratch_base: false, edits: Vec::new() }
}

/// Start from an empty tree and `write` the app file by file, for a
/// shape the blog cannot be edited into, such as an app with no views.
pub fn empty_app() -> Overlay {
    let base = scratch_dir();
    std::fs::create_dir_all(&base).expect("mkdir");
    Overlay { base, scratch_base: true, edits: Vec::new() }
}

pub struct Overlay {
    base: PathBuf,
    /// `base` is this overlay's own scratch tree, removed once copied.
    scratch_base: bool,
    edits: Vec<Edit>,
}

enum Edit {
    Write { path: String, content: String },
    Replace { path: String, find: String, replace: String },
    Remove { path: String },
}

impl Overlay {
    /// Add a file, or replace one outright.
    pub fn write(mut self, path: &str, content: &str) -> Self {
        self.edits.push(Edit::Write { path: path.into(), content: content.into() });
        self
    }

    /// Delete an existing file, such as `db/schema.rb` for an app that
    /// keeps its schema in `db/structure.sql` instead. Panics when it is
    /// absent.
    pub fn remove(mut self, path: &str) -> Self {
        self.edits.push(Edit::Remove { path: path.into() });
        self
    }

    /// Replace the one occurrence of `find` in an existing file. Panics
    /// when `find` is absent or ambiguous, so a fixture that changes
    /// under a test fails at the edit instead of silently testing the
    /// unedited app.
    pub fn edit(mut self, path: &str, find: &str, replace: &str) -> Self {
        self.edits.push(Edit::Replace {
            path: path.into(),
            find: find.into(),
            replace: replace.into(),
        });
        self
    }

    /// Emit, then run one of the emitted test files
    /// (`ruby -Itest -I. <path>` in the emitted tree).
    pub fn run_test(self, test_path: &str) -> Run {
        self.run_test_with(&[], test_path)
    }

    /// `run_test` with every string literal frozen
    /// (`ruby --enable-frozen-string-literal`), as spinel compiles the
    /// tree. A construct whose emitted Ruby mutates a literal passes on
    /// plain CRuby and raises FrozenError in the spinel binary; this
    /// makes the CRuby run fail the same way.
    pub fn run_test_frozen(self, test_path: &str) -> Run {
        self.run_test_with(&["--enable-frozen-string-literal"], test_path)
    }

    fn run_test_with(self, flags: &[&str], test_path: &str) -> Run {
        let (emitted, errors) = self.emit_tree(BuildTarget::Ruby);
        let output = ruby()
            .args(flags)
            .args(["-Itest", "-I."])
            .arg(test_path)
            .current_dir(&emitted)
            .output()
            .expect("spawn ruby");
        let command: Vec<&str> =
            std::iter::once("ruby").chain(flags.iter().copied()).chain(["-Itest", "-I.", test_path]).collect();
        Run::new(command.join(" "), emitted, errors, output)
    }

    /// Emit, then run `script` against the booted app: `main.rb` is
    /// required and the default adapter configured on an in-memory
    /// database before the script's first line.
    pub fn run_ruby(self, script: &str) -> Run {
        let (emitted, errors) = self.emit_tree(BuildTarget::Ruby);
        // `ruby -e` reads its script in the locale's encoding, so under
        // `LANG=C` any non-ASCII literal is a syntax error. The magic
        // comment must be the first line to count.
        let script = format!(
            "# encoding: utf-8\nrequire File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\n{script}"
        );
        let output = ruby()
            .arg("-e")
            .arg(&script)
            .current_dir(&emitted)
            .env("BLOG_DB", ":memory:")
            .output()
            .expect("spawn ruby");
        Run::new("ruby -e <script>".into(), emitted, errors, output)
    }

    /// Compile the unchanged Spinel output and run its native binary.
    /// The consumer boots libraries, not the HTTP server or a database.
    pub fn run_spinel(self, script: &str) -> Run {
        let (emitted, errors) = self.emit_tree(BuildTarget::Spinel);
        std::fs::write(emitted.join("contract.rb"), format!("require_relative \"boot\"\n{script}"))
            .expect("write native consumer");
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let compiled = Command::new(&compiler).args(["contract.rb", "-o", "contract"])
            .current_dir(&emitted).output().expect("spawn spinel");
        std::fs::write(emitted.join("compile.stdout"), &compiled.stdout).expect("write compile stdout");
        std::fs::write(emitted.join("compile.stderr"), &compiled.stderr).expect("write compile stderr");
        if !compiled.status.success() {
            return Run::new(format!("{compiler} contract.rb -o contract"), emitted, errors, compiled);
        }
        let output = Command::new(emitted.join("contract")).current_dir(&emitted)
            .output().expect("run native consumer");
        Run::new(format!("{compiler} contract.rb -o contract && ./contract"), emitted, errors, output)
    }

    /// Copy the fixture, apply the edits, analyze, and write the Ruby
    /// target. Returns the emitted tree, removed with its scratch when
    /// dropped (see `Run`'s `Drop`), and `check`'s error diagnostics.
    pub fn emit(self, target: BuildTarget) -> (Emitted, Vec<String>) {
        let (dir, errors) = self.emit_tree(target);
        (Emitted(dir), errors)
    }

    /// Emit and retain structured errors plus the source catalogue that
    /// resolves each diagnostic's `FileId`. Tests that make provenance
    /// claims should inspect that mapping rather than grep `Debug` spans.
    pub fn emit_with_app(
        self,
        target: BuildTarget,
    ) -> (Emitted, roundhouse::App, Vec<Diagnostic>) {
        let (dir, app, errors) = self.emit_app_tree(target);
        (Emitted(dir), app, errors)
    }

    fn emit_tree(self, target: BuildTarget) -> (PathBuf, Vec<String>) {
        let (emitted, _app, errors) = self.emit_app_tree(target);
        let errors = errors.into_iter().map(|d| format!("{:?}: {}", d.span, d.message)).collect();
        (emitted, errors)
    }

    fn emit_app_tree(self, target: BuildTarget) -> (PathBuf, roundhouse::App, Vec<Diagnostic>) {
        let scratch = scratch_dir();
        let source = scratch.join("app");
        copy_tree(&self.base, &source);
        if self.scratch_base {
            let _ = std::fs::remove_dir_all(&self.base);
        }
        for edit in &self.edits {
            match edit {
                Edit::Write { path, content } => {
                    let full = source.join(path);
                    std::fs::create_dir_all(full.parent().unwrap()).expect("mkdir");
                    std::fs::write(full, content).expect("write overlay file");
                }
                Edit::Replace { path, find, replace } => {
                    let full = source.join(path);
                    let text = std::fs::read_to_string(&full)
                        .unwrap_or_else(|e| panic!("overlay edit: cannot read {path}: {e}"));
                    let hits = text.matches(find.as_str()).count();
                    assert_eq!(
                        hits, 1,
                        "overlay edit: expected exactly one {find:?} in {path}, found {hits}"
                    );
                    std::fs::write(full, text.replacen(find.as_str(), replace, 1))
                        .expect("write overlay edit");
                }
                Edit::Remove { path } => {
                    std::fs::remove_file(source.join(path))
                        .unwrap_or_else(|e| panic!("overlay remove: cannot remove {path}: {e}"));
                }
            }
        }

        let mut app = ingest_app(&source).expect("ingest the overlaid fixture");
        let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
        let mut errors: Vec<_> = diagnose(&app)
            .into_iter()
            .chain(lower_diags)
            .filter(|d| d.severity == Severity::Error)
            .collect();

        let emitted = scratch.join("emitted");
        let (files, emit_diags) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&app, &source, target)
        });
        errors.extend(emit_diags.into_iter().filter(|d| d.severity == Severity::Error));
        let files = files.expect("target files");
        roundhouse::project::write_to_dir(&files, &emitted).expect("write ruby target tree");
        (emitted, app, errors)
    }
}

/// What one emit-and-run produced.
pub struct Run {
    command: String,
    pub emitted: PathBuf,
    /// Analysis, lowering and emission error diagnostics for the overlaid app.
    pub errors: Vec<String>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    fn new(command: String, emitted: PathBuf, errors: Vec<String>, output: std::process::Output) -> Self {
        Run {
            command,
            emitted,
            errors,
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// The whole claim: analysis and emit report no errors AND the emitted
    /// program ran clean. Either half alone is not support.
    pub fn assert_passes(&self) {
        assert!(
            self.errors.is_empty(),
            "analysis or emit reports errors, so the construct is not supported yet:\n{}",
            self.errors.join("\n")
        );
        assert!(
            self.success,
            "analysis and emit are clean but the emitted program failed: `{}` in {}\n\
             \n=== stdout ===\n{}\n=== stderr ===\n{}",
            self.command,
            self.emitted.display(),
            self.stdout,
            self.stderr,
        );
    }
}

/// A run takes its scratch tree with it, unless its test is failing (the
/// failure names the tree) or `ROUNDHOUSE_KEEP_EMITTED=1` keeps every one.
/// A run expected to fail is a passing test and is removed too. Left
/// behind, a full suite fills `/tmp`'s inodes.
impl Drop for Run {
    fn drop(&mut self) {
        remove_scratch(&self.emitted);
    }
}

/// An emitted tree, read as its path; removed like a `Run`'s.
pub struct Emitted(PathBuf);

impl std::ops::Deref for Emitted {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

// So `Command::current_dir(&emitted)` and the other `AsRef<Path>` APIs
// take it as they took the PathBuf it replaced.
impl AsRef<Path> for Emitted {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Emitted {
    fn drop(&mut self) {
        remove_scratch(&self.0);
    }
}

fn remove_scratch(emitted: &Path) {
    if !std::thread::panicking() && std::env::var_os("ROUNDHOUSE_KEEP_EMITTED").is_none() {
        if let Some(scratch) = emitted.parent() {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

/// `ruby`, with the prerequisite checked once and named on failure.
pub fn ruby() -> Command {
    static CHECKED: std::sync::Once = std::sync::Once::new();
    CHECKED.call_once(|| {
        let ok = Command::new("ruby")
            .args(["-rsqlite3", "-e", "1"])
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(
            ok,
            "emit-and-run needs `ruby` with the sqlite3 gem on PATH:\n\n    gem install sqlite3\n"
        );
    });
    Command::new("ruby")
}

/// A fresh directory per run: tests in one binary run in parallel.
fn scratch_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-emit-and-run-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("mkdir");
        for entry in std::fs::read_dir(src).expect("readdir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP.contains(&name.as_ref()) || name.starts_with(".bundle") {
                continue;
            }
            copy_tree(&entry.path(), &dst.join(entry.file_name()));
        }
    } else {
        std::fs::copy(src, dst).expect("copy file");
    }
}
