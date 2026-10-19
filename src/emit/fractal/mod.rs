//! `--target fractal`: compile-time specialization of a running Rails app,
//! one class at a time.
//!
//! Rails keeps running the app. A person names the classes to work on
//! (`--class Shop`); Roundhouse ingests and analyzes the whole app, so it
//! knows what Rails cannot see from one call site (types, effects, which
//! definitions exist), and uses that application knowledge to produce
//! specialized *variants* of the selected classes' methods. Everything
//! else, including every method no pass changes, is the app's own code.
//! Deselecting a class and rebuilding gives the app back.
//!
//! The pieces, in the order a build runs them:
//!
//! - [`select`]: Rubydex finds every body of each selected class,
//!   reopenings included, minus the bodies of other classes nested in it.
//! - [`region`]: each `def` written directly in a selected body, with the
//!   analyzer's facts about it and whether a variant can be attached.
//! - [`pass`]: the transformations. Each proposes edits to a region and
//!   the assumptions they rest on. None is registered yet; this module
//!   is the baseline they attach to.
//! - [`variant`]: the edited method, wrapped so that it keeps its
//!   nesting, file and line, and the generated initializer that installs
//!   it.
//! - `fractal.rb`, the runtime: installs each variant when its owner
//!   loads, only after its guards hold, and leaves the app's own method
//!   in place otherwise.
//!
//! No app file is edited, so the output is an overlay holding only what
//! a build generates (`config/initializers/fractal.rb`, when there is at
//! least one variant) to be placed on the app's root. With no variant it
//! is empty.
//!
//! Where YJIT and ZJIT would want different shapes, Fractal takes ZJIT's.
//! The unit of specialization is a whole method, chosen once at load:
//! a variant is its own ISEQ, so ZJIT compiles, profiles and inlines it
//! without the original's paths in it, and no call pays a guard. See
//! `docs/pipeline/fractal.md`.

pub mod pass;
pub mod region;
pub mod select;
mod overlay;
pub mod variant;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::App;
use crate::span::FileId;

pub use pass::{Cx, Edit, Guard, Pass, Proposal};
pub use region::{Analyzed, Knowledge, Region};
pub use select::Selection;
pub use variant::{Decline, INITIALIZER, RUNTIME, Variant};

/// What a build decided, for the CLI's report and `--report PATH`.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub classes: Vec<String>,
    /// Every body the selection covers, as `Class path:line`.
    pub bodies: Vec<String>,
    pub passes: Vec<String>,
    pub regions: Vec<RegionReport>,
    pub variants: Vec<Variant>,
    pub declined: Vec<Decline>,
}

#[derive(Debug, Serialize)]
pub struct RegionReport {
    pub key: String,
    pub admitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub knowledge: Knowledge,
}

impl Report {
    /// The human-readable summary the CLI prints.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.classes.is_empty() {
            out.push_str("fractal: no --class selected; nothing to generate\n");
            return out;
        }
        for body in &self.bodies {
            out.push_str(&format!("fractal: selected {body}\n"));
        }
        let admitted = self.regions.iter().filter(|r| r.admitted).count();
        let typed = self.regions.iter().filter(|r| r.admitted && r.knowledge.copies > 0 && r.knowledge.typed == r.knowledge.expressions).count();
        out.push_str(&format!(
            "fractal: {} method(s) in the selection, {admitted} admitted ({typed} fully typed); {} pass(es): {}\n",
            self.regions.len(),
            self.passes.len(),
            if self.passes.is_empty() { "none registered".to_string() } else { self.passes.join(", ") },
        ));
        out.push_str(&format!("fractal: {} variant(s)\n", self.variants.len()));
        for r in &self.regions {
            match &r.reason {
                Some(reason) => out.push_str(&format!("  not admitted  {}: {reason}\n", r.key)),
                None if r.knowledge.copies == 0 => out.push_str(&format!("  admitted      {} (not in the analyzed IR)\n", r.key)),
                None => out.push_str(&format!(
                    "  admitted      {} ({}/{} typed)\n",
                    r.key, r.knowledge.typed, r.knowledge.expressions
                )),
            }
        }
        for v in &self.variants {
            out.push_str(&format!("  variant       {} [{}]\n", v.key, v.passes.join(", ")));
        }
        for d in &self.declined {
            out.push_str(&format!("  declined      {} [{}]: {}\n", d.at, d.pass, d.reason));
        }
        out
    }
}

/// A build's decisions and the files it generates.
pub struct Plan {
    pub report: Report,
    pub generated: Vec<(String, String)>,
}

/// Decide a build: select, find regions, run `passes`, and render the
/// initializer when any variant results. `input` is the app root ingest
/// read; `untrusted` names files ingest could only partly read (a
/// `--survey` gap or a syntax error), which keep the app's code.
pub fn plan(app: &App, input: &Path, classes: &[String], untrusted: &[String], passes: &[&dyn Pass]) -> Result<Plan, String> {
    let mut report = Report {
        classes: classes.to_vec(),
        passes: passes.iter().map(|p| p.name().to_string()).collect(),
        ..Report::default()
    };
    if classes.is_empty() {
        return Ok(Plan { report, generated: Vec::new() });
    }
    let selection = Selection::resolve(app, classes)?;
    let source = |file: FileId| (file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i));
    let roots = [input.to_path_buf(), input.canonicalize().unwrap_or_else(|_| input.to_path_buf())];
    let path_of = |file: FileId| -> Option<String> {
        let path = PathBuf::from(&source(file)?.path);
        let candidates = [path.clone(), path.canonicalize().unwrap_or(path)];
        candidates.iter().find_map(|p| roots.iter().find_map(|root| p.strip_prefix(root).ok())).map(|rel| rel.to_string_lossy().replace('\\', "/"))
    };
    let is_untrusted = |path: &str| untrusted.iter().any(|u| is_suffix_path(path, u) || is_suffix_path(u, path));
    for (name, span) in &selection.bodies {
        if let Some(src) = source(span.file) {
            let (line, _) = src.line_col(span.start);
            report.bodies.push(format!("{name} {}:{line}", path_of(span.file).unwrap_or_else(|| src.path.clone())));
        }
    }

    report.bodies.sort();

    let regions = region::find(app, &selection, &path_of, &is_untrusted);
    let cx = Cx { app };
    let mut variants = Vec::new();
    for region in &regions {
        report.regions.push(RegionReport {
            key: region.key(),
            admitted: region.admission.is_ok(),
            reason: region.admission.clone().err(),
            knowledge: region.knowledge.clone(),
        });
        if region.admission.is_err() || passes.is_empty() {
            continue;
        }
        let Some(src) = source(region.file) else { continue };
        let locate = |offset: u32| {
            let (line, col) = src.line_col(offset);
            format!("{}:{line}:{col}", region.path)
        };
        let proposals = passes.iter().map(|p| (p.name(), p.run(&cx, region))).collect();
        if let Some(v) = variant::build(region, &src.text, proposals, &locate, &mut report.declined) {
            variants.push(v);
        }
    }

    let mut generated = Vec::new();
    if !variants.is_empty() {
        let all = app.const_resolver.for_sources(&app.sources).class_bodies();
        if all.iter().any(|(name, _)| name.as_ref() == "Fractal") {
            return Err("the app defines `Fractal`, the constant the fractal runtime needs".into());
        }
        if input.join(INITIALIZER).exists() {
            return Err(format!("{INITIALIZER} already exists in the app"));
        }
        generated.push((INITIALIZER.to_string(), variant::initializer(&variants)));
    }
    report.variants = variants;
    Ok(Plan { report, generated })
}

/// Write the build's overlay to `out`: the plan's generated files and
/// nothing else, after removing what an earlier build wrote there.
/// Returns how many files were written.
pub fn write(out: &Path, plan: &Plan) -> Result<usize, String> {
    overlay::write(out, &plan.generated)
}

pub use overlay::OWNED;

/// Is `rel` the tail of `path` at a path boundary?
fn is_suffix_path(path: &str, rel: &str) -> bool {
    path == rel || path.ends_with(&format!("/{rel}"))
}
