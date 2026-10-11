//! `--target futamura`: Rails → Ruby that keeps the gems.
//!
//! The output is the app's own tree (`project::futamura_files`) with each
//! specialization spliced in at the source span of the expression it
//! replaces, so everything not specialized stays byte for byte the app's.
//! See `docs/pipeline/specialization.md`.
//!
//! Specializations so far:
//! - [`query`]: static Active Record chains → `ActiveRecord::StatementCache`.
//!
//! Generated support lives in one file, `config/initializers/futamura.rb`:
//! the runtime helper and one `Futamura.define` per specialized site.

pub mod query;

use std::collections::BTreeMap;

use crate::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};

const RUNTIME: &str = include_str!("futamura.rb");
pub const INITIALIZER: &str = "config/initializers/futamura.rb";

/// What a futamura emit did, for the CLI's report.
#[derive(Default)]
pub struct Report {
    pub specialized: Vec<String>,
    pub residue: Vec<String>,
}

/// Splice every specialization into `files` (the identity tree).
pub fn specialize(app: &App, files: &mut Vec<(String, String)>) -> Result<Report, String> {
    let spec = query::Specializer::new(app);
    let mut sites = Vec::new();
    let mut residue = Vec::new();
    for c in &app.controllers {
        for item in &c.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                spec.method_body(&action.body, &mut sites, &mut residue);
            }
        }
    }
    for m in &app.models {
        for item in &m.body {
            if let ModelBodyItem::Method { method, .. } = item {
                spec.method_body(&method.body, &mut sites, &mut residue);
            }
        }
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            spec.method_body(&method.body, &mut sites, &mut residue);
        }
    }

    let mut report = Report::default();
    let location = |span: crate::span::Span| {
        let source = &app.sources[span.file.0 as usize - 1];
        let (line, col) = source.line_col(span.start);
        format!("{}:{line}:{col}", source.path)
    };
    // Group by file; splice from the end so earlier offsets stay valid.
    let mut by_file: BTreeMap<u32, Vec<query::Site>> = BTreeMap::new();
    for site in sites {
        by_file.entry(site.span.file.0).or_default().push(site);
    }
    let mut defines = Vec::new();
    for (file, mut sites) in by_file {
        let source = &app.sources[file as usize - 1];
        let Some((_, content)) = files.iter_mut().find(|(p, _)| source.path.ends_with(p.as_str()) && is_suffix_path(&source.path, p)) else {
            report.residue.extend(sites.iter().map(|s| format!("{}: file not in the emitted tree", location(s.span))));
            continue;
        };
        // Spans index the text ingest parsed; splice only into the same text.
        if *content != source.text {
            report.residue.extend(sites.iter().map(|s| format!("{}: source text differs from the file", location(s.span))));
            continue;
        }
        sites.sort_by_key(|s| std::cmp::Reverse(s.span.start));
        let mut last_start = u32::MAX;
        for site in sites {
            if site.span.end > last_start {
                report.residue.push(format!("{}: overlaps another site", location(site.span)));
                continue;
            }
            let (start, end) = (site.span.start as usize, site.span.end as usize);
            let orig = &content[start..end];
            let call = site.call.replace("{orig}", orig);
            content.replace_range(start..end, &call);
            last_start = site.span.start;
            report.specialized.push(location(site.span));
            defines.push(site.define);
        }
    }
    report.residue.extend(residue.into_iter().map(|r| format!("{}: {}", location(r.span), r.reason)));

    if !defines.is_empty() {
        defines.sort();
        let mut init = RUNTIME.to_string();
        init.push('\n');
        let static_scopes = spec.static_association_scopes();
        if !static_scopes.is_empty() {
            let names = static_scopes.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(", ");
            init.push_str(&format!("Futamura::Preload.static_scopes({names})\n\n"));
        }
        init.push_str("Rails.application.config.to_prepare do\n");
        for d in &defines {
            init.push_str("  ");
            init.push_str(d);
            init.push('\n');
        }
        init.push_str("end\n");
        if files.iter().any(|(p, _)| p == INITIALIZER) {
            return Err(format!("{INITIALIZER} already exists in the app"));
        }
        files.push((INITIALIZER.to_string(), init));
        files.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Ok(report)
}

/// Is `rel` the tail of `path` at a path boundary?
fn is_suffix_path(path: &str, rel: &str) -> bool {
    path == rel || path.ends_with(&format!("/{rel}"))
}
