//! `fractal verify`: check that an overlay changes nothing Ruby can observe
//! about the code it compiles.
//!
//! Verify boots the app twice in fresh Ruby processes, before and after
//! copying the overlay onto it, and fingerprints every named module with a
//! method in a compiled file: ancestors, constants, instance variable names,
//! and for each of its methods the owner, visibility, parameters, arity,
//! source location and a digest of its bytecode with column and node
//! positions removed ([`FINGERPRINT`]). The app's files are restored
//! afterwards, whether or not the second boot succeeds.
//!
//! The overlay is verified when the two fingerprints are equal and every
//! rewrite in the build report was observed: some method in both boots is
//! defined on the rewrite's line. A rewrite no method sits on was never
//! checked, so it fails rather than passing vacuously.
//!
//! A pass that lowers an expression to a cheaper one changes the bytecode of
//! the line it rewrites by design ([`super::pass::Pass::changes_bytecode`]). For
//! its rewrites, a method that both boots define in the same place and whose
//! lines span the rewrite observes it, and its bytecode may differ on the
//! rewritten lines only: every other line, and the shape of its
//! instruction sequences, must compile as before. That the rewritten line
//! does what it did rests on the pass, not on this check.
//!
//! Identical bytecode is not the whole story. Constant lookup depends on the
//! lexical scope the code was compiled in, which the bytecode does not
//! record, and code compiled from a string (`module_eval`) has a different
//! scope from a `def` written in the class body. A method that was compiled
//! from a string on one side and from the file on the other, and that names
//! a constant by a relative path, is reported as unverified.
//!
//! On Unix, an interrupt (Ctrl+C) while the overlay is applied stops the
//! Ruby boot but not verify, which still restores the app; elsewhere only
//! the copies in `originals/` protect it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

/// The Ruby script that writes a fingerprint.
pub const FINGERPRINT: &str = include_str!("verify/fingerprint.rb");

/// What to verify and how to boot the app.
pub struct Options {
    pub overlay: PathBuf,
    pub app: PathBuf,
    /// The `--report` JSON of the build that emitted the overlay.
    pub report: PathBuf,
    /// The command that runs Ruby in the app, split into words
    /// (`shadowenv exec -- bundle exec ruby`).
    pub ruby: Vec<String>,
    /// Files to require before the compiled files, relative to the app
    /// (`config/environment`).
    pub requires: Vec<String>,
    /// Fingerprint every named module, not only those with a method in a
    /// compiled file.
    pub all: bool,
    /// Where to keep both fingerprints; a temporary directory otherwise.
    pub keep: Option<PathBuf>,
}

/// What a verification found.
#[derive(Debug, Default)]
pub struct Outcome {
    pub ruby: String,
    pub modules: usize,
    pub methods: usize,
    /// Rewrites some method in both boots is defined on.
    pub observed: Vec<String>,
    /// Rewrites no method is defined on.
    pub unobserved: Vec<String>,
    /// Observable differences between the two boots.
    pub differences: Vec<String>,
    /// Equal fingerprints whose equality does not settle behavior.
    pub unverified: Vec<String>,
}

impl Outcome {
    pub fn verified(&self) -> bool {
        self.unobserved.is_empty() && self.differences.is_empty() && self.unverified.is_empty()
    }

    /// The human-readable summary the CLI prints.
    pub fn render(&self) -> String {
        let mut summary = format!(
            "{}: {} module(s), {} method(s) fingerprinted; {} rewrite(s) observed\n",
            self.ruby,
            self.modules,
            self.methods,
            self.observed.len()
        );
        for (heading, findings) in [("difference", &self.differences), ("unobserved", &self.unobserved), ("unverified", &self.unverified)] {
            for finding in findings {
                summary.push_str(&format!("  {heading:<12}{finding}\n"));
            }
        }
        summary.push_str(if self.verified() {
            "verified: the overlay changes nothing Ruby can observe of the compiled code\n"
        } else {
            "NOT verified\n"
        });
        summary
    }
}

#[derive(Deserialize)]
struct ReportInput {
    compiled: Vec<String>,
    rewritten: Vec<String>,
    rewrites: Vec<RewriteInput>,
}

#[derive(Deserialize)]
struct RewriteInput {
    at: String,
    pass: String,
}

#[derive(Deserialize)]
struct ManifestInput {
    rewritten: Vec<String>,
}

#[derive(Deserialize)]
struct Fingerprint {
    ruby: String,
    anonymous_modules: usize,
    modules: BTreeMap<String, serde_json::Value>,
    methods: BTreeMap<String, MethodPrint>,
}

#[derive(Deserialize)]
struct MethodPrint {
    owner: String,
    visibility: String,
    parameters: String,
    arity: i64,
    original_name: String,
    source_location: Option<String>,
    bytecode: String,
    #[serde(default)]
    evaluated: bool,
    #[serde(default)]
    relative_constants: Vec<String>,
    #[serde(default)]
    listing: Option<String>,
    /// Line => digest of the instructions on it, for a method in a compiled
    /// file.
    #[serde(default)]
    lines: Option<BTreeMap<String, String>>,
    /// Digest of the method's bytecode apart from its instructions.
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    last_line: Option<u32>,
}

/// Verify `options.overlay` against the app it was built from.
pub fn verify(options: &Options) -> Result<Outcome, String> {
    let report: ReportInput = read_json(&options.report)?;
    let manifest: ManifestInput = read_json(&options.overlay.join(super::MANIFEST))?;
    let report_rewritten: BTreeSet<&String> = report.rewritten.iter().collect();
    let manifest_rewritten: BTreeSet<&String> = manifest.rewritten.iter().collect();
    if report_rewritten != manifest_rewritten {
        return Err(format!(
            "the report rewrites {:?} but the overlay's {} lists {:?}; they are not from the same build",
            report.rewritten,
            super::MANIFEST,
            manifest.rewritten
        ));
    }
    if report.rewrites.is_empty() {
        return Err("the report has no rewrites, so there is nothing to verify".into());
    }
    if options.ruby.is_empty() {
        return Err("--ruby is empty".into());
    }
    // Only the report's rewritten files are saved and restored, so the
    // overlay must hold exactly those: an extra file would be copied onto
    // the app and left there, and a missing one would compare the original
    // with itself.
    let overlay_files: BTreeSet<String> =
        super::tree::overlay_files(&options.overlay)?.iter().map(|path| path.to_string_lossy().replace('\\', "/")).collect();
    let extra: Vec<&String> = overlay_files.iter().filter(|path| !report_rewritten.contains(path)).collect();
    let missing: Vec<&&String> = report_rewritten.iter().filter(|path| !overlay_files.contains(path.as_str())).collect();
    if !extra.is_empty() || !missing.is_empty() {
        return Err(format!(
            "the overlay must hold exactly the files the report rewrites; it also holds {extra:?} and lacks {missing:?}"
        ));
    }
    if let Some(keep) = &options.keep
        && super::tree::absolute(keep)?.starts_with(super::tree::absolute(&options.overlay)?)
    {
        return Err(format!("--keep {} is inside the overlay {}; keep the fingerprints outside it", keep.display(), options.overlay.display()));
    }
    let app = options.app.canonicalize().map_err(|error| format!("app {}: {error}", options.app.display()))?;

    static VERIFICATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let number = VERIFICATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = std::env::temp_dir().join(format!("fractal-verify-{}-{number}", std::process::id()));
    let directory = options.keep.clone().unwrap_or_else(|| temporary.clone());
    fs::create_dir_all(&directory).map_err(|error| format!("create {}: {error}", directory.display()))?;
    // Ruby runs from the app's root, so every path handed to it is absolute.
    let directory = directory.canonicalize().map_err(|error| format!("resolve {}: {error}", directory.display()))?;
    let script = directory.join("fingerprint.rb");
    fs::write(&script, FINGERPRINT).map_err(|error| format!("write {}: {error}", script.display()))?;

    let original_path = directory.join("original.json");
    let rewritten_path = directory.join("rewritten.json");
    let boot = |output: &Path| fingerprint(options, &app, &report.compiled, &script, output);
    boot(&original_path)?;
    {
        // While the overlay is applied, an interrupt must not kill this
        // process before it restores the app: it reaches only the Ruby child,
        // whose boot then fails. The originals are also copied to disk, for
        // a process killed outright.
        let _interrupts = HoldInterrupts::new();
        let restore = Restore::save(&app, &report.rewritten, &directory.join("originals"))?;
        super::apply(&options.overlay, &app)?;
        let booted = boot(&rewritten_path);
        restore.finish()?;
        booted?;
    }
    let original: Fingerprint = read_json(&original_path)?;
    let rewritten: Fingerprint = read_json(&rewritten_path)?;
    if options.keep.is_none() {
        let _ = fs::remove_dir_all(&temporary);
    }
    Ok(compare(&original, &rewritten, &report.rewrites))
}

fn read_json<Value: for<'json> Deserialize<'json>>(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))
}

/// Boot the app in a fresh Ruby and write its fingerprint to `output`.
fn fingerprint(options: &Options, app: &Path, compiled: &[String], script: &Path, output: &Path) -> Result<(), String> {
    let _ = fs::remove_file(output);
    let mut command = Command::new(&options.ruby[0]);
    // The child gets the default disposition back even while this process
    // ignores interrupts (see HoldInterrupts), so Ctrl+C still stops a boot.
    // SAFETY: `signal` is async-signal-safe, as code between fork and exec
    // must be.
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            for signal in INTERRUPTS {
                libc::signal(signal, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    command.args(&options.ruby[1..]).arg(script).arg(output).arg(app).current_dir(app);
    for path in &options.requires {
        command.args(["--require", path]);
    }
    for path in compiled {
        command.args(["--file", path]);
    }
    if options.all {
        command.arg("--all");
    }
    let result = command.output().map_err(|error| format!("run {}: {error}", options.ruby.join(" ")))?;
    if !result.status.success() || !output.is_file() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(30).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(format!("fingerprinting the app failed ({}):\n{}", result.status, tail.join("\n")));
    }
    Ok(())
}

/// The signals a terminal sends the whole foreground process group.
#[cfg(unix)]
const INTERRUPTS: [libc::c_int; 2] = [libc::SIGINT, libc::SIGQUIT];

/// Ignores [`INTERRUPTS`] in this process until dropped, as `system(3)`
/// does while its child runs.
#[cfg(unix)]
struct HoldInterrupts {
    previous: Vec<(libc::c_int, libc::sighandler_t)>,
}

/// Off Unix there are no signal dispositions to hold.
#[cfg(not(unix))]
struct HoldInterrupts;

#[cfg(not(unix))]
impl HoldInterrupts {
    fn new() -> Self {
        HoldInterrupts
    }
}

#[cfg(unix)]
impl HoldInterrupts {
    fn new() -> Self {
        // SAFETY: replacing a disposition with SIG_IGN runs no handler code.
        let previous = INTERRUPTS.iter().map(|&signal| (signal, unsafe { libc::signal(signal, libc::SIG_IGN) })).collect();
        HoldInterrupts { previous }
    }
}

#[cfg(unix)]
impl Drop for HoldInterrupts {
    fn drop(&mut self) {
        for &(signal, handler) in &self.previous {
            // SAFETY: puts back the disposition `new` replaced.
            unsafe {
                libc::signal(signal, handler);
            }
        }
    }
}

/// The app's files the overlay replaces, as they were, written back by
/// [`Restore::finish`] or, if that is never reached, on drop.
struct Restore {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    finished: bool,
}

impl Restore {
    /// Read the app's `rewritten` files, and copy each into `backup` at its
    /// app-relative path.
    fn save(app: &Path, rewritten: &[String], backup: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        for relative_path in rewritten {
            let path = app.join(relative_path);
            let contents = match fs::read(&path) {
                Ok(contents) => Some(contents),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("read {}: {error}", path.display())),
            };
            if let Some(contents) = &contents {
                let copy = backup.join(relative_path);
                if let Some(parent) = copy.parent() {
                    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
                }
                fs::write(&copy, contents).map_err(|error| format!("back up {relative_path} to {}: {error}", copy.display()))?;
            }
            files.push((path, contents));
        }
        Ok(Restore { files, finished: false })
    }

    fn restore(&self) -> Result<(), String> {
        for (path, contents) in &self.files {
            match contents {
                Some(contents) => fs::write(path, contents).map_err(|error| format!("restore {}: {error}", path.display()))?,
                None => {
                    let _ = fs::remove_file(path);
                }
            }
        }
        for (path, contents) in &self.files {
            if fs::read(path).ok() != *contents {
                return Err(format!("{} was not restored; restore it with version control", path.display()));
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.finished = true;
        self.restore()
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.restore()
        {
            eprintln!("fractal: {error}");
        }
    }
}

/// `path:line` of a rewrite's `path:line:col`.
fn rewrite_line(at: &str) -> &str {
    at.rsplit_once(':').map_or(at, |(line, _column)| line)
}

fn compare(original: &Fingerprint, rewritten: &Fingerprint, rewrites: &[RewriteInput]) -> Outcome {
    let mut outcome = Outcome {
        ruby: original.ruby.clone(),
        modules: original.modules.len(),
        methods: original.methods.len(),
        ..Outcome::default()
    };
    if original.ruby != rewritten.ruby {
        outcome.differences.push(format!("the two boots ran different Rubies: {} and {}", original.ruby, rewritten.ruby));
    }
    if original.anonymous_modules != rewritten.anonymous_modules {
        outcome.differences.push(format!(
            "anonymous modules with a method in a compiled file: {} before, {} after (anonymous modules are counted, not compared)",
            original.anonymous_modules, rewritten.anonymous_modules
        ));
    }

    let rewrites_by_line: BTreeMap<&str, Vec<&RewriteInput>> = rewrites.iter().fold(BTreeMap::new(), |mut by_line, rewrite| {
        by_line.entry(rewrite_line(&rewrite.at)).or_insert_with(Vec::new).push(rewrite);
        by_line
    });
    let explain = |location: Option<&String>| -> String {
        location
            .and_then(|location| rewrites_by_line.get(location.as_str()))
            .map(|on_line| {
                let sites: Vec<String> = on_line.iter().map(|rewrite| format!("{} [{}]", rewrite.at, rewrite.pass)).collect();
                format!(" (rewritten at {})", sites.join(", "))
            })
            .unwrap_or_default()
    };

    // `path:line` of the rewrites made by passes that change bytecode by
    // design, which a method spanning them may differ on.
    let bytecode_changing: BTreeSet<String> = super::pass::registered().iter().filter(|pass| pass.changes_bytecode()).map(|pass| pass.name().to_string()).collect();
    let lowered_lines: BTreeSet<&str> = rewrites.iter().filter(|rewrite| bytecode_changing.contains(&rewrite.pass)).map(|rewrite| rewrite_line(&rewrite.at)).collect();

    for name in original.modules.keys().chain(rewritten.modules.keys()).collect::<BTreeSet<_>>() {
        match (original.modules.get(name), rewritten.modules.get(name)) {
            (Some(before), Some(after)) if before != after => {
                let fields = before.as_object().into_iter().flatten().filter(|(field, value)| after.get(field.as_str()) != Some(value));
                for (field, value) in fields {
                    outcome.differences.push(format!("{name}: {field} {value} -> {}", after.get(field.as_str()).unwrap_or(&serde_json::Value::Null)));
                }
            }
            (Some(_), None) => outcome.differences.push(format!("{name}: module no longer has a method in a compiled file")),
            (None, Some(_)) => outcome.differences.push(format!("{name}: module newly has a method in a compiled file")),
            _ => {}
        }
    }

    let mut observed_lines = BTreeSet::new();
    for name in original.methods.keys().chain(rewritten.methods.keys()).collect::<BTreeSet<_>>() {
        let (before, after) = match (original.methods.get(name), rewritten.methods.get(name)) {
            (Some(before), Some(after)) => (before, after),
            (Some(before), None) => {
                outcome.differences.push(format!("{name}: missing after the rewrite{}", explain(before.source_location.as_ref())));
                continue;
            }
            (None, Some(after)) => {
                outcome.differences.push(format!("{name}: added by the rewrite{}", explain(after.source_location.as_ref())));
                continue;
            }
            (None, None) => continue,
        };
        if let (Some(before_location), Some(after_location)) = (&before.source_location, &after.source_location)
            && before_location == after_location
        {
            observed_lines.insert(before_location.clone());
        }
        let lowered_here = lowered_within(before, after, &lowered_lines);
        if !lowered_here.is_empty() {
            observed_lines.extend(lowered_here.iter().map(|line| line.to_string()));
        }
        let mut because = explain(before.source_location.as_ref().or(after.source_location.as_ref()));
        if !lowered_here.is_empty() {
            let sites: Vec<String> = rewrites.iter().filter(|rewrite| lowered_here.contains(&rewrite_line(&rewrite.at))).map(|rewrite| format!("{} [{}]", rewrite.at, rewrite.pass)).collect();
            because = format!(" (rewritten at {})", sites.join(", "));
        }
        let fields: [(&str, String, String); 7] = [
            ("owner", before.owner.clone(), after.owner.clone()),
            ("visibility", before.visibility.clone(), after.visibility.clone()),
            ("parameters", before.parameters.clone(), after.parameters.clone()),
            ("arity", before.arity.to_string(), after.arity.to_string()),
            ("original name", before.original_name.clone(), after.original_name.clone()),
            ("source location", format!("{:?}", before.source_location), format!("{:?}", after.source_location)),
            ("bytecode", before.bytecode.clone(), after.bytecode.clone()),
        ];
        for (field, before_value, after_value) in fields {
            if before_value == after_value {
                continue;
            }
            if field == "bytecode" && !lowered_here.is_empty() {
                // The rewritten lines may compile differently; every other
                // line of the method must not.
                outcome.differences.extend(differences_off_lines(name, before, after, &lowered_here, &because));
            } else if field == "bytecode"
                && let (Some(before_listing), Some(after_listing)) = (&before.listing, &after.listing)
            {
                outcome.differences.push(format!("{name}: bytecode differs{because}: {}", first_difference(before_listing, after_listing)));
            } else {
                outcome.differences.push(format!("{name}: {field} {before_value} -> {after_value}{because}"));
            }
        }
        if before.bytecode == after.bytecode && before.evaluated != after.evaluated {
            let constants: BTreeSet<&String> = before.relative_constants.iter().chain(&after.relative_constants).collect();
            if !constants.is_empty() {
                let constants: Vec<&str> = constants.into_iter().map(String::as_str).collect();
                outcome.unverified.push(format!(
                    "{name}: compiled from a string on one side and from the file on the other, and looks up {} by a relative path; \
                     the lexical scope that resolves it is not part of the bytecode{because}",
                    constants.join(", ")
                ));
            }
        }
    }

    for rewrite in rewrites {
        if observed_lines.contains(rewrite_line(&rewrite.at)) {
            outcome.observed.push(rewrite.at.clone());
        } else {
            outcome.unobserved.push(format!(
                "{} [{}]: no method in either boot is defined on this line, so the rewrite was not checked (was the file loaded?)",
                rewrite.at, rewrite.pass
            ));
        }
    }
    outcome
}

/// The `path:line`s of `lowered_lines` inside a method that both boots
/// define in the same place with the same extent.
fn lowered_within<'line>(before: &MethodPrint, after: &MethodPrint, lowered_lines: &BTreeSet<&'line str>) -> Vec<&'line str> {
    let (Some(location), Some(last_line)) = (&before.source_location, before.last_line) else { return Vec::new() };
    if after.source_location.as_ref() != Some(location) || after.last_line != Some(last_line) {
        return Vec::new();
    }
    let Some((path, first_line)) = location.rsplit_once(':').and_then(|(path, line)| Some((path, line.parse::<u32>().ok()?))) else { return Vec::new() };
    lowered_lines
        .iter()
        .copied()
        .filter(|at| at.rsplit_once(':').is_some_and(|(at_path, line)| at_path == path && line.parse::<u32>().is_ok_and(|line| first_line <= line && line <= last_line)))
        .collect()
}

/// Where a method's bytecode differs other than on `lowered` lines: in its
/// shape (a sequence's header or catch table), or on another line.
fn differences_off_lines(name: &str, before: &MethodPrint, after: &MethodPrint, lowered: &[&str], because: &str) -> Vec<String> {
    let (Some(before_lines), Some(after_lines)) = (&before.lines, &after.lines) else {
        return vec![format!("{name}: bytecode differs{because}, and the fingerprint has no per-line bytecode to confine the difference to the rewritten lines")];
    };
    let mut differences = Vec::new();
    if before.shape != after.shape {
        differences.push(format!("{name}: bytecode differs outside the rewritten lines, in a sequence's header or catch table{because}"));
    }
    let rewritten: BTreeSet<&str> = lowered.iter().filter_map(|at| at.rsplit_once(':').map(|(_, line)| line)).collect();
    for line in before_lines.keys().chain(after_lines.keys()).collect::<BTreeSet<_>>() {
        if !rewritten.contains(line.as_str()) && before_lines.get(line) != after_lines.get(line) {
            differences.push(format!("{name}: bytecode differs on line {line}, which no rewrite that changes bytecode is on{because}"));
        }
    }
    differences
}

/// The first place two normalised listings differ, with some context.
fn first_difference(before: &str, after: &str) -> String {
    let start = before.chars().zip(after.chars()).take_while(|(left, right)| left == right).count();
    let excerpt = |text: &str| -> String {
        let characters: Vec<char> = text.chars().collect();
        let from = start.saturating_sub(60);
        let to = (start + 60).min(characters.len());
        characters[from..to].iter().collect()
    };
    format!("before …{}… after …{}…", excerpt(before), excerpt(after))
}
