//! Machine-readable inventory and ranked hotspot report for
//! `roundhouse check`.
//!
//! Personal apps (Jumpstart Pro, client trees) and large corpora share the
//! same peel loop: check → name one wall → fix Roundhouse → re-check. The
//! line-oriented stderr of `check --continue` is enough to gate CI, but it
//! is a poor worklist: agents scrape 1–2k lines to rebuild the tables a
//! human already wants (top codes, ivar names, unmodeled gems, path
//! prefixes) and still lack a snippet or a suggested wall identity.
//!
//! This module turns the same diagnostics + survey gaps into:
//! - a JSON inventory (`--inventory PATH`) agents can cluster without
//!   scraping stderr;
//! - a human hotspot report (`--hotspots`, or auto with `--continue`)
//!   that groups errors by peel identity and shows file:line, message,
//!   a short snippet, and a suggested wall slug.
//!
//! Warnings stay out of the hotspot ranking: they are the coverage ledger,
//! not the peel worklist. Survey gaps appear as their own section.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::gems::GemCensus;
use crate::ingest::IngestError;
use crate::span::SourceFile;
use crate::ty::Ty;

const HOTSPOT_EXAMPLE_LIMIT: usize = 5;
const SNIPPET_MAX_CHARS: usize = 160;

/// One diagnostic (or survey gap) in the inventory dump.
#[derive(Clone, Debug, Serialize)]
pub struct InventoryItem {
    pub severity: &'static str,
    pub code: String,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
}

/// Ranked peel bucket with enough context to start a fix.
#[derive(Debug, Serialize)]
pub struct Hotspot {
    /// Stable slug an agent can put on a causes file / PR title.
    pub identity: String,
    pub code: String,
    pub count: usize,
    pub suggested_wall: String,
    pub examples: Vec<InventoryItem>,
}

#[derive(Debug, Serialize)]
pub struct InventorySummary {
    pub parse_errors: usize,
    pub errors: usize,
    pub warnings: usize,
    pub notes: usize,
    pub survey_gaps: usize,
}

#[derive(Debug, Serialize)]
pub struct InventoryGems {
    pub framework: usize,
    pub modeled: usize,
    pub infrastructure: usize,
    pub in_repo: usize,
    pub unknown: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CheckInventory {
    pub schema: u32,
    pub app: String,
    pub summary: InventorySummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gems: Option<InventoryGems>,
    pub diagnostics: Vec<InventoryItem>,
    pub survey_gaps: Vec<InventoryItem>,
    pub hotspots: Vec<Hotspot>,
}

/// Build the inventory + hotspot ranking from a finished check pass.
pub fn build(
    app_path: &str,
    app_root: &Path,
    sources: &[SourceFile],
    parse_diags: &[Diagnostic],
    diags: &[Diagnostic],
    survey_errors: &[IngestError],
    census: Option<&GemCensus>,
) -> CheckInventory {
    let mut diagnostics: Vec<InventoryItem> = parse_diags
        .iter()
        .chain(diags.iter())
        .map(|d| item_from_diagnostic(d, sources, app_root))
        .collect();
    diagnostics.sort_by(|a, b| {
        (a.severity, &a.code, &a.path, a.line, a.column)
            .cmp(&(b.severity, &b.code, &b.path, b.line, b.column))
    });

    let survey_gaps: Vec<InventoryItem> = survey_errors
        .iter()
        .map(|g| item_from_survey(g, app_root))
        .collect();

    let summary = InventorySummary {
        parse_errors: parse_diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        errors: diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        warnings: diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
        notes: diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .count(),
        survey_gaps: survey_errors.len(),
    };

    let gems = census.map(|c| InventoryGems {
        framework: c.count(crate::gems::GemFate::Framework),
        modeled: c.count(crate::gems::GemFate::Modeled),
        infrastructure: c.count(crate::gems::GemFate::Infrastructure),
        in_repo: c.count(crate::gems::GemFate::InRepo),
        unknown: c.unknown().map(|g| g.name.clone()).collect(),
    });

    let hotspots = rank_hotspots(
        diagnostics
            .iter()
            .filter(|d| d.severity == "error")
            .cloned()
            .collect(),
        &survey_gaps,
    );

    CheckInventory {
        schema: 1,
        app: app_path.to_string(),
        summary,
        gems,
        diagnostics,
        survey_gaps,
        hotspots,
    }
}

/// Human report: ranked hotspots with examples. Printed to stderr.
pub fn render_hotspots(inventory: &CheckInventory) -> String {
    let mut out = String::new();
    if inventory.hotspots.is_empty() {
        out.push_str("── Hotspots: none (no error diagnostics or survey gaps) ──\n");
        return out;
    }
    // Ranked total is Σ hotspot.count (analyze errors + peel-shaped
    // survey gaps), not summary.errors alone — those disagree when
    // parse errors or survey mounts join the ranking.
    let ranked: usize = inventory.hotspots.iter().map(|h| h.count).sum();
    out.push_str(&format!(
        "── Hotspots: {} bucket(s), {} item(s) ranked for peel ({} analyze error(s)) ──\n",
        inventory.hotspots.len(),
        ranked,
        inventory.summary.errors,
    ));
    for (i, bucket) in inventory.hotspots.iter().enumerate() {
        out.push_str(&format!(
            "  {}. [{}×] {}  code={}  wall=`{}`\n",
            i + 1,
            bucket.count,
            bucket.identity,
            bucket.code,
            bucket.suggested_wall,
        ));
        for ex in &bucket.examples {
            let loc = if ex.line == 0 {
                ex.path.clone()
            } else {
                format!("{}:{}:{}", ex.path, ex.line, ex.column)
            };
            out.push_str(&format!("       {loc}\n"));
            out.push_str(&format!("         {}\n", ex.message));
            if let Some(snippet) = &ex.snippet {
                out.push_str(&format!("         | {snippet}\n"));
            }
        }
        if bucket.count > bucket.examples.len() {
            out.push_str(&format!(
                "       … and {} more\n",
                bucket.count - bucket.examples.len()
            ));
        }
    }
    out
}

fn item_from_diagnostic(d: &Diagnostic, sources: &[SourceFile], app_root: &Path) -> InventoryItem {
    let (path, line, column, snippet) = resolve_location(d, sources, app_root);
    let message = strip_app_prefix(&d.message, app_root);
    let wall = wall_for_diagnostic(d, &path);
    InventoryItem {
        severity: severity_name(d.severity),
        code: d.code().to_string(),
        path,
        line,
        column,
        message,
        snippet,
        wall,
    }
}

fn item_from_survey(gap: &IngestError, app_root: &Path) -> InventoryItem {
    let (file, message) = match gap {
        IngestError::Unsupported { file, message } | IngestError::Parse { file, message } => {
            (file.clone(), message.clone())
        }
        IngestError::Io(err) => ("<io>".into(), err.to_string()),
    };
    let path = relative_path(app_root, &file);
    let message = strip_app_prefix(&message, app_root);
    let wall = wall_for_survey(&message, &path);
    InventoryItem {
        severity: "survey",
        code: "survey_gap".into(),
        path,
        line: 0,
        column: 0,
        message,
        snippet: None,
        wall,
    }
}

fn resolve_location(
    d: &Diagnostic,
    sources: &[SourceFile],
    app_root: &Path,
) -> (String, u32, u32, Option<String>) {
    let resolved = (d.span.file.0 as usize)
        .checked_sub(1)
        .and_then(|i| sources.get(i))
        .filter(|_| !d.span.is_synthetic());
    match resolved {
        Some(sf) => {
            let (line, column) = sf.line_col(d.span.start);
            let path = relative_path(app_root, &sf.path);
            let snippet = line_snippet(&sf.text, line);
            (path, line, column, snippet)
        }
        None => ("<synthetic>".into(), 0, 0, None),
    }
}

fn line_snippet(text: &str, line: u32) -> Option<String> {
    if line == 0 {
        return None;
    }
    let content = text.lines().nth(line as usize - 1)?.trim();
    if content.is_empty() {
        return None;
    }
    Some(truncate(content, SNIPPET_MAX_CHARS))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn relative_path(app_root: &Path, path: &str) -> String {
    let p = Path::new(path);
    p.strip_prefix(app_root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn strip_app_prefix(text: &str, app_root: &Path) -> String {
    let abs = app_root.to_string_lossy();
    text.replace(abs.as_ref(), "<APP>")
        .replace(&abs.replace('/', "\\"), "<APP>")
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn rank_hotspots(errors: Vec<InventoryItem>, survey_gaps: &[InventoryItem]) -> Vec<Hotspot> {
    let mut groups: BTreeMap<(String, String), Vec<InventoryItem>> = BTreeMap::new();
    for item in errors {
        let identity = item
            .wall
            .clone()
            .unwrap_or_else(|| format!("{}:{}", item.code, generic_shape(&item.message)));
        groups
            .entry((identity, item.code.clone()))
            .or_default()
            .push(item);
    }
    // Survey gaps that name a peel shape join the ranking so mounts /
    // macros are visible beside analyze errors.
    for item in survey_gaps {
        if let Some(wall) = &item.wall {
            groups
                .entry((wall.clone(), item.code.clone()))
                .or_default()
                .push(item.clone());
        }
    }

    let mut hotspots: Vec<Hotspot> = groups
        .into_iter()
        .map(|((identity, code), mut items)| {
            items.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
            let suggested_wall = items
                .first()
                .and_then(|i| i.wall.clone())
                .unwrap_or_else(|| identity.clone());
            let count = items.len();
            items.truncate(HOTSPOT_EXAMPLE_LIMIT);
            Hotspot {
                identity,
                code,
                count,
                suggested_wall,
                examples: items,
            }
        })
        .collect();
    hotspots.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.identity.cmp(&b.identity)));
    hotspots
}

/// Wall identity from structured [`DiagnosticKind`] fields. Analyze
/// diagnostics must not scrape `message` — peels grow kinds, not English.
fn wall_for_diagnostic(d: &Diagnostic, path: &str) -> Option<String> {
    match &d.kind {
        DiagnosticKind::IvarUnresolved { name } => Some(ivar_wall(name.as_str())),
        DiagnosticKind::Unsupported { construct, detail, .. } => {
            wall_from_unsupported(construct.as_str(), detail, path)
        }
        DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
            wall_from_send(method.as_str(), recv_ty, path)
        }
        _ => None,
    }
}

/// Survey gaps have no [`DiagnosticKind`] — message heuristics only here.
fn wall_for_survey(message: &str, path: &str) -> Option<String> {
    wall_from_unsupported_text(message, path).or_else(|| {
        if let Some(name) = constant_fqn_from_text(message) {
            Some(unsupported_constant_wall(&name))
        } else {
            None
        }
    })
}

fn ivar_wall(name: &str) -> String {
    let at = if name.starts_with('@') {
        name.to_string()
    } else {
        format!("@{name}")
    };
    let billing = matches!(
        at.as_str(),
        "@subscription"
            | "@subscriptions"
            | "@plan"
            | "@current_plan"
            | "@invoice"
            | "@stripe_invoice"
            | "@payment_processor"
            | "@charge"
            | "@charges"
            | "@checkout_session"
    );
    if billing {
        format!("billing-ivar-{at}")
    } else {
        format!("ivar-{at}")
    }
}

fn wall_from_unsupported(construct: &str, detail: &str, path: &str) -> Option<String> {
    if let Some(wall) = wall_from_unsupported_text(construct, path)
        .or_else(|| wall_from_unsupported_text(detail, path))
    {
        return Some(wall);
    }
    if construct == "constant" {
        if let Some(name) = constant_fqn_from_detail(detail) {
            return Some(unsupported_constant_wall(&name));
        }
    }
    None
}

/// Shared peel-shaped heuristics over free text (unsupported construct /
/// detail, or survey-gap messages).
fn wall_from_unsupported_text(text: &str, path: &str) -> Option<String> {
    if text.contains("unmodeled gem `pay`") || text.contains("Pay::") {
        return Some("pay-unmodeled".into());
    }
    if text.contains("route mount") || text.contains("only top-level literal engine mounts") {
        return Some("engine-route-mount".into());
    }
    if path.contains("application_client")
        || text.contains("Net::HTTP")
        || text.contains("Net::Protocol")
    {
        return Some("application-client-net-http".into());
    }
    if text.contains("generated column") || text.contains("PostgreSQL cast") {
        return Some("schema-generated-cast".into());
    }
    if text.contains("rate_limit") {
        return Some("controller-macro-rate-limit".into());
    }
    None
}

fn wall_from_send(method: &str, recv_ty: &Ty, path: &str) -> Option<String> {
    let recv = match recv_ty {
        Ty::Class { id, .. } => id.0.as_str(),
        _ => "",
    };
    if recv.starts_with("Noticed::") || path.contains("notification") {
        return Some("noticed-relation-api".into());
    }
    if method == "payment_processor" || recv.contains("payment_processor") {
        return Some("pay-payment-processor".into());
    }
    None
}

fn unsupported_constant_wall(name: &str) -> String {
    // Keep `::` as `-` so the slug stays path-safe / URL-safe while
    // preserving nested constant identity
    // (`Net::HTTP::Patch` → `unsupported-constant-Net-HTTP-Patch`).
    format!("unsupported-constant-{}", name.replace("::", "-"))
}

/// FQN from an unsupported-constant *detail* (`Pay::Error — …`).
fn constant_fqn_from_detail(detail: &str) -> Option<String> {
    let candidate = detail
        .split(" — ")
        .next()
        .unwrap_or(detail)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':' && c != '_');
    normalize_constant_fqn(candidate)
}

/// FQN from a full survey / legacy message
/// (`constant not supported (all targets): Net::HTTP::Patch — …`).
fn constant_fqn_from_text(message: &str) -> Option<String> {
    if !message.contains("constant not supported") {
        return None;
    }
    let after_paren = message
        .rsplit_once("): ")
        .map(|(_, rest)| rest)
        .unwrap_or(message);
    constant_fqn_from_detail(after_paren)
}

fn normalize_constant_fqn(candidate: &str) -> Option<String> {
    if candidate.is_empty() {
        return None;
    }
    if !candidate
        .split("::")
        .all(|part| !part.is_empty() && part.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
    {
        return None;
    }
    Some(candidate.to_string())
}

fn generic_shape(message: &str) -> String {
    let shape: String = message
        .chars()
        .take(48)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    shape.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
    use crate::ident::Symbol;
    use crate::span::{FileId, Span};

    #[test]
    fn billing_ivar_gets_a_stable_wall_slug() {
        let d = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 0,
                end: 1,
            },
            kind: DiagnosticKind::IvarUnresolved {
                name: Symbol::from("subscription"),
            },
            severity: Severity::Error,
            message: "@subscription has no known type".into(),
        };
        assert_eq!(
            wall_for_diagnostic(&d, "x.rb").as_deref(),
            Some("billing-ivar-@subscription")
        );
    }

    #[test]
    fn pay_constant_maps_to_pay_unmodeled() {
        let d = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 0,
                end: 1,
            },
            kind: DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("constant"),
                detail: "Pay::Error — unmodeled gem `pay`".into(),
            },
            severity: Severity::Error,
            message: "constant not supported (all targets): Pay::Error — unmodeled gem `pay`"
                .into(),
        };
        assert_eq!(
            wall_for_diagnostic(&d, "app/controllers/checkouts_controller.rb").as_deref(),
            Some("pay-unmodeled")
        );
    }

    #[test]
    fn nested_unsupported_constant_keeps_fqn_in_wall_slug() {
        // Net::HTTP* still maps to the application-client wall when the
        // detail names the family; use an unrelated nested constant so
        // the FQN slug path is what we assert.
        let d = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 0,
                end: 1,
            },
            kind: DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("constant"),
                detail: "OmniAuth::Strategies::Developer — unmodeled gem `omniauth`".into(),
            },
            severity: Severity::Error,
            message:
                "constant not supported (all targets): OmniAuth::Strategies::Developer — unmodeled gem `omniauth`"
                    .into(),
        };
        assert_eq!(
            wall_for_diagnostic(&d, "app/controllers/sessions_controller.rb").as_deref(),
            Some("unsupported-constant-OmniAuth-Strategies-Developer")
        );
        assert_eq!(
            constant_fqn_from_text(
                "constant not supported (all targets): Net::HTTP::Patch — missing stdlib surface"
            )
            .as_deref(),
            Some("Net::HTTP::Patch")
        );
        assert_eq!(
            constant_fqn_from_detail("Net::HTTP::Patch — missing stdlib surface").as_deref(),
            Some("Net::HTTP::Patch")
        );
    }

    #[test]
    fn hotspot_header_ranks_sum_not_analyze_errors_alone() {
        let sources = vec![SourceFile {
            path: "/app/config/routes/jumpstart.rb".into(),
            text: "mount Jumpstart::Engine, at: \"/jumpstart\"\n".into(),
        }];
        let d = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 0,
                end: 40,
            },
            kind: DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("route mount"),
                detail: "only top-level literal engine mounts are composed".into(),
            },
            severity: Severity::Error,
            message: "route mount: only top-level literal engine mounts are composed".into(),
        };
        let survey = IngestError::Unsupported {
            file: "/app/config/routes/jumpstart.rb".into(),
            message: "route mount: only top-level literal engine mounts are composed".into(),
        };
        let inv = build(
            "/app",
            Path::new("/app"),
            &sources,
            &[],
            &[d],
            &[survey],
            None,
        );
        assert_eq!(inv.summary.errors, 1);
        let ranked: usize = inv.hotspots.iter().map(|h| h.count).sum();
        assert!(ranked >= 2, "survey gap must join ranking: {ranked}");
        let rendered = render_hotspots(&inv);
        assert!(
            rendered.contains(&format!("{ranked} item(s) ranked")),
            "header must use Σ hotspot counts, got:\n{rendered}"
        );
        assert!(rendered.contains("1 analyze error(s)"), "{rendered}");
    }

    #[test]
    fn hotspots_rank_by_count_and_keep_snippets() {
        let sources = vec![SourceFile {
            path: "/app/lib/jumpstart/app/views/billing/show.html.erb".into(),
            text: "<%= @subscription.name %>\n<%= @plan.amount %>\n".into(),
        }];
        let d1 = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 4,
                end: 17,
            },
            kind: DiagnosticKind::IvarUnresolved {
                name: crate::ident::Symbol::from("subscription"),
            },
            severity: Severity::Error,
            message: "@subscription has no known type".into(),
        };
        let d2 = Diagnostic {
            span: Span {
                file: FileId(1),
                start: 28,
                end: 33,
            },
            kind: DiagnosticKind::IvarUnresolved {
                name: crate::ident::Symbol::from("plan"),
            },
            severity: Severity::Error,
            message: "@plan has no known type".into(),
        };
        let inv = build(
            "/app",
            Path::new("/app"),
            &sources,
            &[],
            &[d1, d2],
            &[],
            None,
        );
        assert_eq!(inv.summary.errors, 2);
        assert_eq!(inv.hotspots.len(), 2);
        assert_eq!(inv.hotspots[0].count, 1);
        assert!(inv.hotspots.iter().any(|h| h.suggested_wall.contains("billing-ivar")));
        assert!(inv.hotspots.iter().any(|h| {
            h.examples
                .iter()
                .any(|e| e.snippet.as_deref() == Some("<%= @subscription.name %>"))
        }));
        let json = serde_json::to_string_pretty(&inv).expect("serialize");
        assert!(json.contains("\"schema\": 1"));
        assert!(json.contains("billing-ivar-@subscription"));
    }
}
