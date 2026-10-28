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

use super::pass::{Context, Pass, Unit};
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
pub fn stages(app: &App, context: &Context<'_>, files: &[String], untrusted: &[String], passes: &[&dyn Pass]) -> Result<Stages, String> {
    let mut stages = Stages {
        passes: passes.iter().map(|pass| PassSummary { name: pass.name().to_string(), summary: pass.summary().to_string() }).collect(),
        ..Stages::default()
    };
    let selection = Selection::resolve(app, context, files)?;
    let source_of = |file: crate::span::FileId| (file.0 as usize).checked_sub(1).and_then(|index| app.sources.get(index));
    let source_files: Vec<StageFile> = selection
        .files
        .iter()
        .filter_map(|(file, path)| source_of(*file).map(|source| StageFile { path: path.clone(), content: source.text.clone() }))
        .collect();
    stages.stages.push(Stage {
        pass: "source".to_string(),
        summary: "The files the build compiles, as the app wrote them.".to_string(),
        files: source_files,
        rewrites: Vec::new(),
        declined: Vec::new(),
    });
    for pass in passes {
        stages.stages.push(Stage {
            pass: pass.name().to_string(),
            summary: pass.summary().to_string(),
            files: Vec::new(),
            rewrites: Vec::new(),
            declined: Vec::new(),
        });
    }
    for (file, path) in &selection.files {
        let Some(source) = source_of(*file) else { continue };
        let skipped = untrusted.iter().any(|untrusted_path| super::is_suffix_path(&source.path, untrusted_path) || super::is_suffix_path(untrusted_path, &source.path));
        if skipped {
            stages.skipped.push(super::Skipped {
                file: path.clone(),
                reason: "ingest could not read the file faithfully (a survey gap or a syntax error)".to_string(),
            });
            for stage in stages.stages.iter_mut().skip(1) {
                stage.files.push(StageFile { path: path.clone(), content: source.text.clone() });
            }
            continue;
        }
        let methods = region::find(app, &selection, *file, path);
        let unit = Unit { file: *file, path: path.clone(), text: &source.text, bodies: selection.bodies_in(*file), methods, selection: &selection };
        // Each pass proposes against the source, as in a build.
        let proposals: Vec<_> = passes.iter().map(|pass| (pass.name(), pass.run(context, &unit))).collect();
        for count in 1..=passes.len() {
            let applied: Vec<_> = proposals.iter().take(count).map(|(name, proposal)| (*name, proposal.clone())).collect();
            let outcome = rewrite::apply(&unit, applied);
            let stage = &mut stages.stages[count];
            stage.files.push(StageFile { path: path.clone(), content: outcome.text.unwrap_or_else(|| source.text.clone()) });
            let name = passes[count - 1].name();
            stage.rewrites.extend(outcome.rewrites.into_iter().filter(|rewrite| rewrite.pass == name));
            stage.declined.extend(outcome.declined.into_iter().filter(|decline| decline.pass == name));
        }
    }
    Ok(stages)
}
