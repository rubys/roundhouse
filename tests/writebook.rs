//! Pinned Writebook inventory. This is deliberately not a conformance test:
//! it guards Roundhouse's view of an external corpus while allowing gaps to
//! disappear. See docs/writebook.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use roundhouse::analyze::{Analyzer, Severity, diagnose};
use roundhouse::diagnostic::Diagnostic;
use roundhouse::ingest::{IngestError, ingest_app, survey};
use serde::{Deserialize, Serialize};

const BASELINE: &str = include_str!("fixtures/writebook-inventory.json");

#[derive(Debug, Serialize, Deserialize)]
struct Inventory {
    schema: u32,
    corpus: Corpus,
    diagnostics: BTreeMap<String, usize>,
    ingest_gaps: BTreeMap<String, usize>,
    lowering: BTreeMap<String, usize>,
    emit: BTreeMap<String, BTreeMap<String, usize>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Corpus {
    models: Vec<String>,
    library_classes: Vec<String>,
    controllers: Vec<String>,
    routes: Vec<String>,
    direct_helpers: Vec<String>,
    views: Vec<String>,
    test_modules: Vec<String>,
    test_cases: Vec<String>,
    fixtures: Vec<String>,
    sources: Vec<String>,
}

fn relative(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .unwrap_or_else(|_| Path::new(path))
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalized_text(root: &Path, text: &str) -> String {
    text.replace(&root.to_string_lossy().replace('\\', "/"), "<APP>")
        .replace(&root.to_string_lossy().replace('/', "\\"), "<APP>")
}

fn diagnostic_key(d: &Diagnostic, app: &roundhouse::app::App, root: &Path) -> String {
    let location = d
        .span
        .file
        .0
        .checked_sub(1)
        .and_then(|index| app.sources.get(index as usize))
        .map(|source| {
            let (line, column) = source.line_col(d.span.start);
            format!("{}:{line}:{column}", relative(root, &source.path))
        })
        .unwrap_or_else(|| "<synthetic>".to_string());
    format!(
        "{:?}|{}|{}|{}",
        d.severity,
        d.code(),
        location,
        normalized_text(root, &d.message)
    )
}

fn multiset<T: IntoIterator<Item = String>>(items: T) -> BTreeMap<String, usize> {
    let mut result = BTreeMap::new();
    for item in items {
        *result.entry(item).or_default() += 1;
    }
    result
}

fn gap_key(gap: &IngestError, root: &Path) -> String {
    match gap {
        IngestError::Unsupported { file, message } | IngestError::Parse { file, message } => {
            format!(
                "{}|{}",
                relative(root, file),
                normalized_text(root, message)
            )
        }
        IngestError::Io(error) => format!("<io>|{error}"),
    }
}

fn route_identities(app: &roundhouse::App) -> (Vec<String>, Vec<String>) {
    let routes = roundhouse::lower::flatten_routes(app)
        .into_iter()
        .map(|r| {
            format!(
                "{:?} {} {}#{} | {}({}) named={}",
                r.method,
                r.path,
                r.controller.0,
                r.action,
                r.as_name,
                r.path_params.join(","),
                r.named
            )
        })
        .collect();
    let direct_helpers = app
        .routes
        .direct_helpers
        .iter()
        .map(|helper| {
            format!(
                "{}({})",
                helper.name,
                helper
                    .params
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    (routes, direct_helpers)
}

fn inventory(root: &Path) -> Inventory {
    survey::activate();
    let (result, parse_diags) = roundhouse::ingest::prism::scope(|| ingest_app(root));
    let gaps = survey::drain();
    let mut app = result.expect("Writebook ingest must complete in survey mode");
    let parse_errors: Vec<_> = parse_diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        parse_errors.is_empty(),
        "Prism parse errors: {parse_errors:#?}"
    );

    let mut lowered = app.clone();
    let lowering = roundhouse::session::analyze_and_lower(&mut lowered);
    let emit = [
        roundhouse::project::BuildTarget::Ruby,
        roundhouse::project::BuildTarget::Spinel,
    ]
    .into_iter()
    .map(|target| {
        let (result, diags) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&lowered, root, target)
        });
        result.expect("Writebook emit inventory must complete without writing output");
        (
            target.as_str().to_string(),
            multiset(diags.iter().map(|d| diagnostic_key(d, &lowered, root))),
        )
    })
    .collect();

    Analyzer::new(&app).analyze(&mut app);
    let mut diagnostics = diagnose(&app);
    roundhouse::analyze::attribution::attribute_ingest_gaps(&mut diagnostics, &app, &gaps);
    roundhouse::analyze::attribution::attribute_unknown_gems(&mut diagnostics, &app);

    let (routes, direct_helpers) = route_identities(&app);
    let mut corpus = Corpus {
        models: app.models.iter().map(|x| x.name.0.to_string()).collect(),
        library_classes: app
            .library_classes
            .iter()
            .map(|x| x.name.0.to_string())
            .collect(),
        controllers: app
            .controllers
            .iter()
            .map(|x| x.name.0.to_string())
            .collect(),
        routes,
        direct_helpers,
        views: app
            .views
            .iter()
            .map(|x| format!("{}.{}", x.name, x.format))
            .collect(),
        test_modules: app
            .test_modules
            .iter()
            .map(|x| x.name.0.to_string())
            .collect(),
        test_cases: app
            .test_modules
            .iter()
            .flat_map(|module| {
                module
                    .tests
                    .iter()
                    .map(move |test| format!("{}|{}", module.name.0, test.name))
            })
            .collect(),
        fixtures: app.fixtures.iter().map(|x| x.path.to_string()).collect(),
        sources: app
            .sources
            .iter()
            .map(|x| relative(root, &x.path))
            .collect(),
    };
    corpus.models.sort();
    corpus.library_classes.sort();
    corpus.controllers.sort();
    corpus.routes.sort();
    corpus.direct_helpers.sort();
    corpus.views.sort();
    corpus.test_modules.sort();
    corpus.test_cases.sort();
    corpus.fixtures.sort();
    corpus.sources.sort();

    Inventory {
        schema: 1,
        corpus,
        diagnostics: multiset(diagnostics.iter().map(|d| diagnostic_key(d, &app, root))),
        ingest_gaps: multiset(gaps.iter().map(|g| gap_key(g, root))),
        lowering: multiset(lowering.iter().map(|d| diagnostic_key(d, &lowered, root))),
        emit,
    }
}

fn assert_no_new(
    label: &str,
    current: &BTreeMap<String, usize>,
    baseline: &BTreeMap<String, usize>,
) {
    let new: Vec<_> = current
        .iter()
        .filter(|(key, count)| **count > baseline.get(*key).copied().unwrap_or(0))
        .map(|(key, count)| format!("{count}× {key}"))
        .collect();
    assert!(
        new.is_empty(),
        "new {label} inventory entries:\n{}",
        new.join("\n")
    );
}

fn assert_not_lost(label: &str, current: &[String], baseline: &[String]) {
    let missing: Vec<_> = baseline
        .iter()
        .filter(|item| !current.contains(item))
        .collect();
    assert!(
        missing.is_empty(),
        "lost {label} corpus identities: {missing:#?}"
    );
}

#[test]
#[ignore = "requires pinned external Writebook checkout; see docs/writebook.md"]
fn pinned_writebook_inventory() {
    let root = std::env::var_os("WRITEBOOK_ROOT")
        .map(PathBuf::from)
        .expect("WRITEBOOK_ROOT is required when this ignored test is explicitly run");
    assert!(
        root.join("app").is_dir(),
        "WRITEBOOK_ROOT is not a Rails app: {}",
        root.display()
    );
    let current = inventory(&root);
    let report = format!("{}\n", serde_json::to_string_pretty(&current).unwrap());
    if let Some(path) = std::env::var_os("WRITEBOOK_INVENTORY_REPORT") {
        std::fs::write(path, &report).expect("write current Writebook inventory report");
    }

    if std::env::var("ROUNDHOUSE_REFRESH_WRITEBOOK_INVENTORY").as_deref() == Ok("1") {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/writebook-inventory.json");
        std::fs::write(&path, &report).unwrap();
        eprintln!("refreshed {}", path.display());
        return;
    }

    let baseline: Inventory = serde_json::from_str(BASELINE).expect("valid Writebook baseline");
    assert_eq!(current.schema, baseline.schema);
    assert_no_new("diagnostic", &current.diagnostics, &baseline.diagnostics);
    assert_no_new("ingest gap", &current.ingest_gaps, &baseline.ingest_gaps);
    assert_no_new("lowering diagnostic", &current.lowering, &baseline.lowering);
    for (target, diagnostics) in &current.emit {
        assert_no_new(
            &format!("{target} emit diagnostic"),
            diagnostics,
            &baseline.emit[target],
        );
    }
    assert_not_lost("model", &current.corpus.models, &baseline.corpus.models);
    assert_not_lost(
        "library class",
        &current.corpus.library_classes,
        &baseline.corpus.library_classes,
    );
    assert_not_lost(
        "controller",
        &current.corpus.controllers,
        &baseline.corpus.controllers,
    );
    assert_not_lost("route", &current.corpus.routes, &baseline.corpus.routes);
    assert_not_lost(
        "direct helper",
        &current.corpus.direct_helpers,
        &baseline.corpus.direct_helpers,
    );
    assert_not_lost("view", &current.corpus.views, &baseline.corpus.views);
    assert_not_lost(
        "test module",
        &current.corpus.test_modules,
        &baseline.corpus.test_modules,
    );
    assert_not_lost(
        "test case",
        &current.corpus.test_cases,
        &baseline.corpus.test_cases,
    );
    assert_not_lost(
        "fixture",
        &current.corpus.fixtures,
        &baseline.corpus.fixtures,
    );
    assert_not_lost(
        "source file",
        &current.corpus.sources,
        &baseline.corpus.sources,
    );
}

#[test]
fn inventory_rejects_replacing_one_gap_with_another_at_the_same_total() {
    let baseline = multiset(["old gap".to_string()]);
    let current = multiset(["new gap".to_string()]);
    assert!(std::panic::catch_unwind(|| assert_no_new("gap", &current, &baseline)).is_err());
    let repeated = multiset(["old gap".to_string(), "old gap".to_string()]);
    assert!(std::panic::catch_unwind(|| assert_no_new("gap", &repeated, &baseline)).is_err());
    assert_no_new("gap", &BTreeMap::new(), &baseline);
}

#[test]
fn inventory_rejects_losing_one_source_even_when_another_is_added() {
    let baseline = vec!["app/models/page.rb".to_string()];
    let current = vec!["app/models/another.rb".to_string()];
    assert!(std::panic::catch_unwind(|| assert_not_lost("source", &current, &baseline)).is_err());
}

#[test]
fn route_inventory_retains_direct_helpers_separately_from_dispatch_routes() {
    let tree = [(
        PathBuf::from("config/routes.rb"),
        br#"Rails.application.routes.draw do
  resources :pages, only: :show
  post "/join/:code", to: "pages#create"
  direct :page_slug do |page, options|
    route_for :page, page, options
  end
end
"#
        .to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest routes");
    let (routes, helpers) = route_identities(&app);
    assert_eq!(
        routes,
        [
            "Get /pages/:id PagesController#show | page(id) named=true",
            "Post /join/:code PagesController#create | create(code) named=false",
        ]
    );
    assert_eq!(helpers, ["page_slug(page,options)"]);
    app.routes.direct_helpers.clear();
    let (current_routes, current_helpers) = route_identities(&app);
    assert_eq!(current_routes, routes);
    assert!(
        std::panic::catch_unwind(|| assert_not_lost("direct helper", &current_helpers, &helpers))
            .is_err()
    );
}
