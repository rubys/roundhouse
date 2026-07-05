//! Gap attribution: reclassify diagnostics whose root cause is a
//! roundhouse ingest gap, not a defect in the user's code.
//!
//! Survey-mode ingest (`ingest::survey`) recovers from unsupported
//! constructs by recording a gap and substituting a `nil` placeholder.
//! That keeps the app queryable, but the analysis downstream of the
//! placeholder is built on sand: the enclosing class's ivars don't
//! resolve, its methods vanish from dispatch, and every consumer of
//! that class inherits the confusion. On a 75K-LOC app (Mastodon) the
//! result is ~1400 `error`-severity diagnostics of which ~90% are
//! shadows of ~150 recorded gaps — each one an accusation ("@account
//! has no known type") pointed at code that is fine.
//!
//! This pass runs after [`super::diagnose`] wherever survey gaps are
//! in hand (roundhouse-check --continue, the LSP, the MCP server) and
//! downgrades the shadowed diagnostics to [`Severity::Info`] with the
//! root cause appended, leaving genuine findings at their original
//! severity. Three attribution rules, cheapest first:
//!
//! 1. **Same file** — the diagnostic sits in a file that recorded a
//!    gap. Whatever the analyzer failed to resolve there, the skipped
//!    construct is the prime suspect.
//! 2. **Receiver class** — a `SendDispatchFailed` whose receiver class
//!    is defined in a gap file: the method likely exists but its
//!    definition (or the DSL declaring it) didn't ingest.
//! 3. **View feeders** — a diagnostic in a view any of whose feeding
//!    controllers (per [`App::view_feeders`], ancestors included) is
//!    tainted: the ivar channel that seeds the view runs through the
//!    gap. A Rails-convention path fallback covers views whose feeder
//!    didn't ingest at all (a wholly-skipped controller file never
//!    registers, so no feeder edge exists to consult).
//!
//! Deliberately over-broad in the safe direction: a genuine app error
//! inside a gap-touched blast radius renders as a coverage note until
//! the gap is fixed — cheap compared to the trust cost of a false
//! accusation. Only unresolved-shaped kinds (`IvarUnresolved`,
//! `SendDispatchFailed`, `IncompatibleBinop`, `UnresolvedType`) are
//! eligible; `Parse` (a real syntax error), `Unsupported` (already a
//! tool statement), and `GradualUntyped` (author-signed) never move.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::expr::Expr;
use crate::ident::ClassId;
use crate::ingest::{survey, IngestError};
use crate::span::FileId;
use crate::ty::Ty;

/// Downgrade diagnostics attributable to `gaps` (see module docs).
/// No-op when `gaps` is empty — strict-mode callers can pass through
/// unconditionally.
pub fn attribute_ingest_gaps(diags: &mut [Diagnostic], app: &App, gaps: &[IngestError]) {
    if gaps.is_empty() || diags.is_empty() {
        return;
    }
    let ctx = AttributionCtx::build(app, gaps);
    for d in diags {
        if !eligible(&d.kind) {
            continue;
        }
        if let Some(cause) = ctx.cause_for(d) {
            d.severity = Severity::Info;
            d.message.push_str(&format!(
                " — likely roundhouse coverage, not an app error (ingest gap in {})",
                cause
            ));
        }
    }
}

/// Kinds that mean "the analyzer could not resolve this" — the shapes a
/// nil-placeholder substitution produces downstream. `Parse`,
/// `Unsupported`, and `GradualUntyped` describe their own root cause and
/// are never reattributed.
fn eligible(kind: &DiagnosticKind) -> bool {
    matches!(
        kind,
        DiagnosticKind::IvarUnresolved { .. }
            | DiagnosticKind::SendDispatchFailed { .. }
            | DiagnosticKind::IncompatibleBinop { .. }
            | DiagnosticKind::UnresolvedType { .. }
    )
}

/// Everything precomputed once per attribution run.
struct AttributionCtx<'a> {
    app: &'a App,
    /// Gap file path → rendered cause (`path (bucketed message)`), app-root
    /// relative for readability. First gap per file wins — one cause line
    /// is enough to point the user at the file.
    gap_by_path: HashMap<&'a str, String>,
    /// `FileId`s of sources whose path recorded a gap.
    tainted_files: HashMap<FileId, &'a str>,
    /// Class → the gap path tainting it (own file, or for controllers an
    /// ancestor's file — `ApplicationController` gaps taint every child's
    /// ivar environment through the inherited filter chain).
    tainted_classes: HashMap<ClassId, &'a str>,
    /// View name for each template file, so a diagnostic's `FileId` finds
    /// its view (and through it, its feeders).
    view_by_file: HashMap<FileId, &'a crate::ident::Symbol>,
    /// Whether any controller at all is tainted — the layout rule
    /// (layouts are fed by every controller).
    any_controller_tainted: Option<&'a str>,
}

impl<'a> AttributionCtx<'a> {
    fn build(app: &'a App, gaps: &'a [IngestError]) -> Self {
        let mut gap_by_path: HashMap<&str, String> = HashMap::new();
        for gap in gaps {
            let (IngestError::Unsupported { file, .. } | IngestError::Parse { file, .. }) = gap
            else {
                continue;
            };
            gap_by_path.entry(file).or_insert_with(|| {
                let rel = file.strip_prefix(&app.root).map(|r| r.trim_start_matches('/'));
                format!("{}: {}", rel.unwrap_or(file), survey::bucket_key(gap))
            });
        }

        let mut tainted_files: HashMap<FileId, &str> = HashMap::new();
        for (i, src) in app.sources.iter().enumerate() {
            if let Some((path, _)) = gap_by_path.get_key_value(src.path.as_str()) {
                tainted_files.insert(FileId(i as u32 + 1), path);
            }
        }

        // Class taint by defining file. A class's file is where its first
        // real-span body lands — models/controllers/library classes all
        // ingest single-file in Rails convention.
        let mut class_file: HashMap<ClassId, FileId> = HashMap::new();
        for c in &app.controllers {
            let bodies: Vec<&Expr> = c.actions().map(|a| &a.body).collect();
            if let Some(f) = first_real_file(&bodies) {
                class_file.insert(c.name.clone(), f);
            }
        }
        for m in &app.models {
            let bodies: Vec<&Expr> = m
                .methods()
                .map(|me| &me.body)
                .chain(m.scopes().map(|s| &s.body))
                .collect();
            if let Some(f) = first_real_file(&bodies) {
                class_file.insert(m.name.clone(), f);
            }
        }
        for lc in &app.library_classes {
            let bodies: Vec<&Expr> = lc.methods.iter().map(|me| &me.body).collect();
            if let Some(f) = first_real_file(&bodies) {
                class_file.insert(lc.name.clone(), f);
            }
        }

        let own_taint = |id: &ClassId| -> Option<&str> {
            class_file.get(id).and_then(|f| tainted_files.get(f)).copied()
        };
        let mut tainted_classes: HashMap<ClassId, &str> = HashMap::new();
        for id in class_file.keys() {
            if let Some(p) = own_taint(id) {
                tainted_classes.insert(id.clone(), p);
            }
        }
        // Controllers inherit taint down the parent chain (a gap in
        // ApplicationController's before_action targets poisons every
        // subclass's seeded ivars).
        let parent: HashMap<&ClassId, &ClassId> = app
            .controllers
            .iter()
            .filter_map(|c| c.parent.as_ref().map(|p| (&c.name, p)))
            .collect();
        for c in &app.controllers {
            if tainted_classes.contains_key(&c.name) {
                continue;
            }
            let mut walk = Some(&c.name);
            let mut seen: HashSet<&ClassId> = HashSet::new();
            while let Some(id) = walk {
                if !seen.insert(id) {
                    break;
                }
                if let Some(p) = tainted_classes.get(id).copied().or_else(|| own_taint(id)) {
                    tainted_classes.insert(c.name.clone(), p);
                    break;
                }
                walk = parent.get(id).copied();
            }
        }

        let mut view_by_file: HashMap<FileId, &crate::ident::Symbol> = HashMap::new();
        for v in &app.views {
            if let Some(f) = first_real_file(&[&v.body]) {
                view_by_file.entry(f).or_insert(&v.name);
            }
        }

        let any_controller_tainted = app
            .controllers
            .iter()
            .find_map(|c| tainted_classes.get(&c.name))
            .copied();

        AttributionCtx {
            app,
            gap_by_path,
            tainted_files,
            tainted_classes,
            view_by_file,
            any_controller_tainted,
        }
    }

    /// The rendered cause when `d` is attributable to a gap, else `None`.
    fn cause_for(&self, d: &Diagnostic) -> Option<&String> {
        // Rule 1: the diagnostic's own file recorded a gap.
        if let Some(path) = self.tainted_files.get(&d.span.file) {
            return self.gap_by_path.get(path);
        }
        // Rule 2: dispatch failed on a receiver whose class is tainted.
        if let DiagnosticKind::SendDispatchFailed { recv_ty, .. } = &d.kind {
            if let Some(path) = self.recv_taint(recv_ty) {
                return self.gap_by_path.get(path);
            }
        }
        // Rule 3: the diagnostic sits in a view fed by a tainted
        // controller (layouts: fed by all).
        let view = self.view_by_file.get(&d.span.file)?;
        if view.as_str().starts_with("layouts/") {
            return self.any_controller_tainted.and_then(|p| self.gap_by_path.get(p));
        }
        if let Some(feeders) = self.app.view_feeders.get(view) {
            if let Some(path) = feeders.iter().find_map(|f| self.tainted_classes.get(f)) {
                return self.gap_by_path.get(*path);
            }
        }
        // Fallback for views with no (surviving) feeder: Rails
        // convention maps the view directory to its controller path —
        // covers controllers whose file skipped ingest wholesale and so
        // never registered a feeder edge. `application_controller.rb`
        // participates as the conventional root of every chain.
        for cand in conventional_controller_paths(view.as_str()) {
            if let Some(cause) = self
                .gap_by_path
                .iter()
                .find_map(|(p, c)| p.ends_with(&cand).then_some(c))
            {
                return Some(cause);
            }
        }
        None
    }

    /// Taint for a dispatch receiver: the root class of `ty` (unions:
    /// any arm) defined in a gap file.
    fn recv_taint(&self, ty: &Ty) -> Option<&&str> {
        match ty {
            Ty::Class { id, .. } => self.tainted_classes.get(id),
            Ty::Union { variants } => variants.iter().find_map(|v| self.recv_taint(v)),
            _ => None,
        }
    }
}

/// The first non-synthetic file any of these bodies' subtrees touches.
fn first_real_file(bodies: &[&Expr]) -> Option<FileId> {
    fn find(e: &Expr) -> Option<FileId> {
        if !e.span.is_synthetic() {
            return Some(e.span.file);
        }
        let mut found = None;
        e.node.for_each_child(&mut |c| {
            if found.is_none() {
                found = find(c);
            }
        });
        found
    }
    bodies.iter().find_map(|b| find(b))
}

/// Candidate controller path suffixes for a view name, most specific
/// first: `admin/reports/show` → `app/controllers/admin/reports_controller.rb`,
/// `app/controllers/admin_controller.rb`, then the conventional root
/// `app/controllers/application_controller.rb`.
fn conventional_controller_paths(view: &str) -> Vec<String> {
    let mut out = Vec::new();
    let dirs: Vec<&str> = view.split('/').collect();
    if dirs.len() >= 2 {
        for i in (1..dirs.len()).rev() {
            out.push(format!("app/controllers/{}_controller.rb", dirs[..i].join("/")));
        }
    }
    out.push("app/controllers/application_controller.rb".to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;
    use crate::span::Span;

    fn diag(kind: DiagnosticKind, severity: Severity, file: FileId) -> Diagnostic {
        Diagnostic {
            span: Span { file, start: 0, end: 1 },
            severity,
            message: "test".to_string(),
            kind,
        }
    }

    #[test]
    fn no_gaps_is_a_noop() {
        let app = App::new();
        let mut diags = vec![diag(
            DiagnosticKind::IvarUnresolved { name: Symbol::from("x") },
            Severity::Error,
            FileId(1),
        )];
        attribute_ingest_gaps(&mut diags, &app, &[]);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn same_file_gap_downgrades_to_info_with_cause() {
        let mut app = App::new();
        app.root = "fixtures/demo".to_string();
        app.sources.push(crate::span::SourceFile {
            path: "fixtures/demo/app/models/thing.rb".to_string(),
            text: "class Thing; end".to_string(),
        });
        let gaps = vec![IngestError::Unsupported {
            file: "fixtures/demo/app/models/thing.rb".to_string(),
            message: "unsupported expression node: SingletonClassNode (…)".to_string(),
        }];
        let mut diags = vec![
            diag(
                DiagnosticKind::IvarUnresolved { name: Symbol::from("x") },
                Severity::Error,
                FileId(1),
            ),
            // Parse diagnostics never move, even in a tainted file.
            diag(
                DiagnosticKind::Parse { message: "boom".to_string() },
                Severity::Error,
                FileId(1),
            ),
        ];
        attribute_ingest_gaps(&mut diags, &app, &gaps);
        assert_eq!(diags[0].severity, Severity::Info);
        assert!(
            diags[0].message.contains("app/models/thing.rb"),
            "cause names the gap file: {}",
            diags[0].message
        );
        assert!(diags[0].message.contains("SingletonClassNode"));
        assert_eq!(diags[1].severity, Severity::Error);
    }

    #[test]
    fn untainted_file_keeps_severity() {
        let mut app = App::new();
        app.sources.push(crate::span::SourceFile {
            path: "app/models/clean.rb".to_string(),
            text: String::new(),
        });
        let gaps = vec![IngestError::Unsupported {
            file: "app/models/other.rb".to_string(),
            message: "gap".to_string(),
        }];
        let mut diags = vec![diag(
            DiagnosticKind::IvarUnresolved { name: Symbol::from("x") },
            Severity::Error,
            FileId(1),
        )];
        attribute_ingest_gaps(&mut diags, &app, &gaps);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn conventional_paths_walk_up_the_view_dirs() {
        assert_eq!(
            conventional_controller_paths("admin/reports/show"),
            vec![
                "app/controllers/admin/reports_controller.rb".to_string(),
                "app/controllers/admin_controller.rb".to_string(),
                "app/controllers/application_controller.rb".to_string(),
            ]
        );
        assert_eq!(
            conventional_controller_paths("layouts/application"),
            vec![
                "app/controllers/layouts_controller.rb".to_string(),
                "app/controllers/application_controller.rb".to_string(),
            ]
        );
    }
}
