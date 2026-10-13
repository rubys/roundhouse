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
pub mod routes;

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

const BRIDGE: &str = include_str!("bridge.rb");
pub const ROUTES_INITIALIZER: &str = "config/initializers/futamura_routes.rb";
/// Where the `--target ruby` emit goes in the futamura tree: outside
/// `app/` and `lib/`, so Rails' autoloader never sees it.
pub const LOWERED_DIR: &str = "vendor/futamura/lowered";

/// The `--target ruby` side of whole-route replacement: the app after
/// `analyze_and_lower`, the files that target emits, and every
/// diagnostic it reported (`project::futamura_whole_route`).
pub struct WholeRoute {
    pub lowered: App,
    pub files: Vec<(String, String)>,
    pub diags: Vec<crate::diagnostic::Diagnostic>,
}

/// Add whole-route replacement to `files`: the lowered tree, the bridge,
/// and an initializer naming the actions `routes::plan` cleared. With no
/// action cleared nothing is added, and the app stays plain Rails.
pub fn whole_route(files: &mut Vec<(String, String)>, wr: &WholeRoute) -> Result<routes::RoutePlan, String> {
    let plan = routes::plan(&wr.lowered, &wr.diags);
    if plan.actions.is_empty() {
        return Ok(plan);
    }
    if files.iter().any(|(p, _)| p == ROUTES_INITIALIZER || p.starts_with("vendor/futamura/")) {
        return Err(format!("{ROUTES_INITIALIZER} or vendor/futamura/ already exists in the app"));
    }
    for (path, content) in &wr.files {
        // The lowered app's own tests and tooling are not part of the
        // served program.
        if path.starts_with("test/") || path.starts_with("e2e/") || path.starts_with("tools/") {
            continue;
        }
        files.push((format!("{LOWERED_DIR}/{path}"), content.clone()));
    }
    files.push(("vendor/futamura/bridge.rb".to_string(), BRIDGE.to_string()));
    let mut init = String::from(
        "# Generated by roundhouse --target futamura: the GET actions whole-route\n\
         # replacement serves from the lowered app (vendor/futamura/bridge.rb).\n\
         # Every other route is Rails'. Requires Ruby 4.1 with RUBY_BOX=1;\n\
         # FUTAMURA_WHOLE_ROUTE=off serves everything from Rails.\n\
         require_relative \"../../vendor/futamura/bridge\"\n\n\
         Futamura::WholeRoute.install(\n  Rails.application,\n  File.expand_path(\"../../vendor/futamura/lowered\", __dir__),\n  %w[\n",
    );
    for a in &plan.actions {
        init.push_str("    ");
        init.push_str(a);
        init.push('\n');
    }
    init.push_str("  ]\n)\n");
    files.push((ROUTES_INITIALIZER.to_string(), init));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(plan)
}
