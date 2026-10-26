//! The compiled files after each pass, for the browser pass explorer
//! (`wasm/fractal/`). A view of a build, not a build: it runs the same
//! [`Selection`], [`region`] and [`rewrite::apply`] a build runs, over an
//! app already in memory, and keeps what each pass added.
//!
//! Stage 0 is the source. Stage *n* is every compiled file with the
//! proposals of passes `1..=n` applied together, the way the driver
//! applies them, with the rewrites and declines pass *n* contributed.

use serde::Serialize;

use crate::App;

use super::pass::{Cx, Pass, Unit};
use super::region;
use super::rewrite::{self, Decline, Rewrite};
use super::select::Selection;

#[derive(Debug, Default, Serialize)]
pub struct Stages {
    /// The registered passes, in order, with what each does.
    pub passes: Vec<PassSummary>,
    pub stages: Vec<Stage>,
    /// Compiled files no pass may rewrite, with why.
    pub skipped: Vec<super::Skipped>,
}

#[derive(Debug, Serialize)]
pub struct PassSummary {
    pub name: String,
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct Stage {
    /// `source`, or the pass's name.
    pub pass: String,
    pub summary: String,
    /// The compiled files as they stand after this stage.
    pub files: Vec<StageFile>,
    /// The edits this pass made.
    pub rewrites: Vec<Rewrite>,
    /// The sites this pass looked at and left alone, and proposals the
    /// driver refused on its behalf.
    pub declined: Vec<Decline>,
}

#[derive(Debug, Serialize)]
pub struct StageFile {
    pub path: String,
    pub content: String,
}

/// The stages of compiling `files` (paths relative to the app root) with
/// `passes`. `untrusted` names files ingest could only partly read; they
/// are reported as skipped and stay the source in every stage.
pub fn stages(app: &App, cx: &Cx<'_>, files: &[String], untrusted: &[String], passes: &[&dyn Pass]) -> Result<Stages, String> {
    let mut out = Stages {
        passes: passes.iter().map(|p| PassSummary { name: p.name().to_string(), summary: p.summary().to_string() }).collect(),
        ..Stages::default()
    };
    let selection = Selection::resolve(app, cx.root, files)?;
    let source_of = |file: crate::span::FileId| (file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i));
    let source_files: Vec<StageFile> = selection
        .files
        .iter()
        .filter_map(|(file, path)| source_of(*file).map(|s| StageFile { path: path.clone(), content: s.text.clone() }))
        .collect();
    out.stages.push(Stage {
        pass: "source".to_string(),
        summary: "The files the build compiles, as the app wrote them.".to_string(),
        files: source_files,
        rewrites: Vec::new(),
        declined: Vec::new(),
    });
    for pass in passes {
        out.stages.push(Stage {
            pass: pass.name().to_string(),
            summary: pass.summary().to_string(),
            files: Vec::new(),
            rewrites: Vec::new(),
            declined: Vec::new(),
        });
    }
    for (file, path) in &selection.files {
        let Some(src) = source_of(*file) else { continue };
        let skipped = untrusted.iter().any(|u| super::is_suffix_path(&src.path, u) || super::is_suffix_path(u, &src.path));
        if skipped {
            out.skipped.push(super::Skipped {
                file: path.clone(),
                reason: "ingest could not read the file faithfully (a survey gap or a syntax error)".to_string(),
            });
            for stage in out.stages.iter_mut().skip(1) {
                stage.files.push(StageFile { path: path.clone(), content: src.text.clone() });
            }
            continue;
        }
        let methods = region::find(app, &selection, *file, path);
        let unit = Unit { file: *file, path: path.clone(), text: &src.text, bodies: selection.bodies_in(*file), methods, selection: &selection };
        // Each pass proposes against the source, as in a build.
        let proposals: Vec<_> = passes.iter().map(|p| (p.name(), p.run(cx, &unit))).collect();
        for n in 1..=passes.len() {
            let upto: Vec<_> = proposals.iter().take(n).map(|(name, p)| (*name, p.clone())).collect();
            let outcome = rewrite::apply(&unit, upto);
            let stage = &mut out.stages[n];
            stage.files.push(StageFile { path: path.clone(), content: outcome.text.unwrap_or_else(|| src.text.clone()) });
            let name = passes[n - 1].name();
            stage.rewrites.extend(outcome.rewrites.into_iter().filter(|r| r.pass == name));
            stage.declined.extend(outcome.declined.into_iter().filter(|d| d.pass == name));
        }
    }
    Ok(out)
}
