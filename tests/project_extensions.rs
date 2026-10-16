//! Project extensions (`roundhouse.yml` `extensions:`): gem and DSL facts an
//! app keeps in its own tree.
//!
//! The facts belong to the app that lists them. They are loaded at ingest,
//! carried on `App::extensions`, and read from the app in hand by analysis
//! and emission, so two apps never see each other's facts, whatever order
//! or thread they are processed in. An app that lists none is unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Barrier;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::{ingest_app, ingest_app_from_tree, IngestError};
use roundhouse::project::{self, BuildTarget};
use roundhouse::App;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const INVOICE_SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"invoices\", force: :cascade do |t|\n    t.decimal \"amount\"\n    t.decimal \"total\"\n  end\nend\n";

/// A model whose gem DSL turns two decimal columns into value objects.
const INVOICE: &str = "class Invoice < ApplicationRecord\n  price_column :amount, currency: :usd\n  bag_columns(attributes: [:total], packer: :default)\n\n  def self.labels\n    invoice = find_by(id: 1)\n    [invoice.amount.currency, invoice.total.parts]\n  end\nend\n";

const PRICE_MACROS: &str = "attribute_macros:\n  - {name: price_column, positional: true}\n  - {name: bag_columns, hash_key: attributes}\n";

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect()
}

fn ingest(files: &[(&str, &str)]) -> Result<App, IngestError> {
    ingest_app_from_tree(tree(files))
}

fn analyzed(files: &[(&str, &str)]) -> App {
    let mut app = ingest(files).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn config_error(files: &[(&str, &str)]) -> (String, String) {
    match ingest(files) {
        Err(IngestError::Parse { file, message }) => (file, message),
        other => panic!("expected a configuration error, got {:?}", other.map(|_| "an app")),
    }
}

fn emitted(app: &App, target: BuildTarget) -> Vec<(String, String)> {
    project::target_files(app, Path::new("."), target).expect("emit")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("no {path} emitted")).1
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| format!("{:?} {}: {}", d.severity, d.code(), d.message)).collect()
}

fn invoice_app(extensions: &[(&str, &str)]) -> App {
    let mut files = vec![
        ("db/schema.rb", INVOICE_SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/invoice.rb", INVOICE),
    ];
    files.extend_from_slice(extensions);
    analyzed(&files)
}

fn dispatch_failures(app: &App) -> Vec<String> {
    codes(&diagnose(app).into_iter().filter(|d| d.code() == "send_dispatch_failed").collect::<Vec<_>>())
}

// ── Configuration ────────────────────────────────────────────────────

#[test]
fn an_app_without_extensions_is_unchanged() {
    let bare = analyzed(&[("app/models/application_record.rb", APPLICATION_RECORD), ("app/models/report.rb", "class Report\nend\n")]);
    let listed_none = analyzed(&[
        ("roundhouse.yml", "extensions: []\n"),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/report.rb", "class Report\nend\n"),
    ]);
    assert!(bare.extensions.is_empty());
    assert!(listed_none.extensions.is_empty());
    assert!(!serde_json::to_string(&bare).unwrap().contains("\"extensions\""), "an empty table is not serialized");
    assert_eq!(emitted(&bare, BuildTarget::Ruby), emitted(&listed_none, BuildTarget::Ruby));
}

#[test]
fn extensions_load_from_roundhouse_yml_in_listing_order() {
    let app = ingest(&[
        ("roundhouse.yml", "test_paths: [test/unit]\nextensions:\n  - config/rh/second\n  - ./config/rh/first\n"),
        ("config/rh/second/gems.yml", "runtime_gems:\n  - {constant: Second, gem: second}\n"),
        ("config/rh/first/gems.yml", "runtime_gems:\n  - {constant: First, gem: first}\n  - {constant: Third, gem: third}\n"),
        ("config/rh/first/macros.yml", PRICE_MACROS),
    ])
    .expect("ingest");
    let gems: Vec<&str> = app.extensions.runtime_gems.iter().map(|row| row.gem.as_str()).collect();
    assert_eq!(gems, ["second", "first", "third"]);
    assert_eq!(app.extensions.attribute_macros.len(), 2);
}

#[test]
fn bad_extensions_entries_are_refused_with_the_config_path() {
    for (config, wanted) in [
        ("extensions: [\"../shared\"]\n", "extensions entries must be non-empty app-relative directories without '..': \"../shared\""),
        ("extensions: [\"/opt/shared\"]\n", "extensions entries must be non-empty app-relative directories without '..': \"/opt/shared\""),
        ("extensions: [\"\"]\n", "extensions entries must be non-empty app-relative directories without '..': \"\""),
        ("extensions: [ext, ./ext]\n", "extensions entries must be listed once: \"./ext\""),
        ("extensions: ext\n", "invalid type"),
        ("extension: [ext]\n", "unknown field `extension`"),
    ] {
        let (file, message) = config_error(&[("roundhouse.yml", config), ("ext/gems.yml", "")]);
        assert_eq!(file, "roundhouse.yml", "{config}");
        assert!(message.contains(wanted), "{config}: {message}");
    }
}

#[test]
fn a_malformed_extension_names_its_own_file() {
    let (file, message) = config_error(&[("roundhouse.yml", "extensions: [missing]\n")]);
    assert_eq!((file.as_str(), message.as_str()), ("missing", "not a directory"));

    let (file, message) = config_error(&[
        ("roundhouse.yml", "extensions: [ext]\n"),
        ("ext/gems.yml", "runtime_gems:\n  - {gem: widgets}\n"),
    ]);
    assert_eq!(file, "ext/gems.yml");
    assert!(message.contains("needs exactly one of constant and any_text"), "{message}");

    let (file, message) = config_error(&[("roundhouse.yml", "extensions: [ext]\n"), ("ext/types/widget.rbs", "class Widget\nend\n")]);
    assert_eq!(file, "ext/types");
    assert!(message.contains("types is not supported"), "{message}");
}

#[test]
fn a_rails_dependent_gem_another_fact_loads_is_refused_across_extensions() {
    let (file, message) = config_error(&[
        ("roundhouse.yml", "extensions: [one, two]\n"),
        ("one/gems.yml", "rails_dependent:\n  - {gem: widget-engine, why: requiring it loads Rails}\n"),
        ("two/gems.yml", "runtime_gems:\n  - {constant: WidgetEngine, gem: widget-engine}\n"),
    ]);
    assert_eq!(file, "one/gems.yml");
    assert!(message.contains("\"widget-engine\" is rails_dependent but the tree also loads it (runtime_gems in two/gems.yml)"), "{message}");
}

#[cfg(unix)]
#[test]
fn symbolic_links_to_extension_directories_are_refused() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("roundhouse_extension_directory_links_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = root.join("app");
    let outside = root.join("outside");
    std::fs::create_dir_all(app.join("config")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    symlink(&outside, app.join("config/linked")).unwrap();
    std::fs::write(app.join("roundhouse.yml"), "extensions: [config/linked]\n").unwrap();
    let Err(IngestError::Parse { file, .. }) = ingest_app(&app) else { panic!("a linked extension directory must fail") };
    assert!(file.ends_with("roundhouse.yml"), "{file}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn symbolic_links_to_extension_files_load_shared_facts() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("roundhouse_extension_file_links_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = root.join("app");
    let outside = root.join("outside");
    std::fs::create_dir_all(app.join("config/ext")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("gems.yml"), "runtime_gems:\n  - {constant: Widget, gem: widgets}\n").unwrap();
    std::fs::write(outside.join("macros.yml"), PRICE_MACROS).unwrap();
    symlink(outside.join("gems.yml"), app.join("config/ext/gems.yml")).unwrap();
    symlink("../../../outside/macros.yml", app.join("config/ext/macros.yml")).unwrap();
    std::fs::write(app.join("roundhouse.yml"), "extensions: [config/ext]\n").unwrap();

    let app = ingest_app(&app).expect("linked extension files must load");
    let gems: Vec<&str> = app.extensions.runtime_gems.iter().map(|row| row.gem.as_str()).collect();
    assert_eq!(gems, ["widgets"]);
    let macros: Vec<&str> = app.extensions.attribute_macros.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(macros, ["price_column", "bag_columns"]);
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn dangling_extension_file_links_are_errors_not_absent_files() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("roundhouse_extension_dangling_links_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("config/ext")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("roundhouse.yml"), "extensions: [config/ext]\n").unwrap();

    for name in ["gems.yml", "macros.yml"] {
        let path = root.join("config/ext").join(name);
        symlink("missing.yml", &path).unwrap();
        let Err(IngestError::Parse { file, .. }) = ingest_app(&root) else { panic!("a dangling {name} link must fail") };
        assert_eq!(file, path.display().to_string());
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::remove_dir_all(&root).unwrap();
}

// ── Attribute macros ─────────────────────────────────────────────────

#[test]
fn without_the_macros_every_value_object_read_fails_dispatch() {
    let app = invoice_app(&[]);
    let failures = dispatch_failures(&app);
    assert_eq!(failures.len(), 2, "{failures:#?}");
}

#[test]
fn attribute_macros_type_the_attributes_and_refuse_the_declaration() {
    let app = invoice_app(&[("roundhouse.yml", "extensions: [config/rh]\n"), ("config/rh/macros.yml", PRICE_MACROS)]);
    let diagnostics = diagnose(&app);
    assert!(dispatch_failures(&app).is_empty(), "{:#?}", codes(&diagnostics));

    // The gap is not hidden: no target lowers the DSL, so each declaration
    // is an error, and each value-object read is a gradual escape.
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    let refused: Vec<String> = errors
        .iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::Unsupported { construct, detail, .. } if detail.contains("from a project extension") => {
                Some(construct.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(refused, ["price_column", "bag_columns"], "{:#?}", codes(&diagnostics));
    assert_eq!(errors.len(), 2, "{:#?}", codes(&diagnostics));
    let gradual = diagnostics.iter().filter(|d| d.code() == "gradual_untyped").count();
    assert_eq!(gradual, 2, "{:#?}", codes(&diagnostics));
}

#[test]
fn an_undeclared_attribute_keeps_its_column_type() {
    let macros = "attribute_macros:\n  - {name: price_column, positional: true}\n";
    let app = invoice_app(&[("roundhouse.yml", "extensions: [ext]\n"), ("ext/macros.yml", macros)]);
    let failures = dispatch_failures(&app);
    assert_eq!(failures.len(), 1, "only `total` is still a number: {failures:#?}");
    assert!(failures[0].contains("`parts`"), "{failures:#?}");
}

#[test]
fn concern_attribute_macros_type_each_includer_and_refuse_the_declarations() {
    let invoice = INVOICE.replace(
        "  price_column :amount, currency: :usd\n  bag_columns(attributes: [:total], packer: :default)\n",
        "  include Priced\n",
    );
    let receipt = invoice.replace(
        "class Invoice < ApplicationRecord\n",
        "class Receipt < ApplicationRecord\n  self.table_name = \"invoices\"\n",
    );
    let app = analyzed(&[
        ("roundhouse.yml", "extensions: [config/rh]\n"),
        ("config/rh/macros.yml", PRICE_MACROS),
        ("db/schema.rb", INVOICE_SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/concerns/priced.rb", "module Priced\n  extend ActiveSupport::Concern\n  included do\n    price_column :amount, currency: :usd\n    bag_columns(attributes: [:total], packer: :default)\n  end\nend\n"),
        ("app/models/invoice.rb", &invoice),
        ("app/models/receipt.rb", &receipt),
    ]);
    let diagnostics = diagnose(&app);
    assert!(dispatch_failures(&app).is_empty(), "{:#?}", codes(&diagnostics));
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 4, "{:#?}", codes(&diagnostics));
    for owner in ["Invoice", "Receipt"] {
        for name in ["price_column", "bag_columns"] {
            let refusal = errors.iter().find(|d| matches!(&d.kind,
                DiagnosticKind::Unsupported { construct, detail, .. }
                    if construct.as_str() == name && detail.contains(&format!("`{owner}`"))
            )).unwrap_or_else(|| panic!("missing {owner}.{name} refusal: {:#?}", codes(&diagnostics)));
            let source = &app.sources[refusal.span.file.0 as usize - 1];
            assert!(source.path.ends_with("app/models/concerns/priced.rb"));
        }
    }
    assert_eq!(diagnostics.iter().filter(|d| d.code() == "gradual_untyped").count(), 4);
}

#[test]
fn dormant_concern_attribute_macros_do_not_change_column_types_or_report_declarations() {
    let invoice = INVOICE.replace(
        "  price_column :amount, currency: :usd\n  bag_columns(attributes: [:total], packer: :default)\n",
        "",
    );
    let app = analyzed(&[
        ("roundhouse.yml", "extensions: [config/rh]\n"),
        ("config/rh/macros.yml", PRICE_MACROS),
        ("db/schema.rb", INVOICE_SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/concerns/priced.rb", "module Priced\n  extend ActiveSupport::Concern\n  included do\n    price_column :amount\n    bag_columns(attributes: [:total])\n  end\nend\n"),
        ("app/models/invoice.rb", &invoice),
    ]);
    let diagnostics = diagnose(&app);
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 2, "{:#?}", codes(&diagnostics));
    assert!(errors.iter().all(|d| d.code() == "send_dispatch_failed"), "{:#?}", codes(&diagnostics));
    assert!(!diagnostics.iter().any(|d| d.code() == "gradual_untyped"), "{:#?}", codes(&diagnostics));
}

// ── Gem facts on the emitted trees ──────────────────────────────────

const ENGINE_LOCK: &str = "GEM\n  specs:\n    widget-engine (2.0.0)\n\nDEPENDENCIES\n  widget-engine\n";
const ENGINE_REPORT: &str = "class Report\n  def self.client\n    WidgetEngine::Client.new\n  end\nend\n";

fn engine_files(rails_dependent: bool) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![
        ("Gemfile.lock", ENGINE_LOCK),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/report.rb", ENGINE_REPORT),
    ];
    if rails_dependent {
        files.push(("roundhouse.yml", "extensions: [config/rh]\n"));
        files.push(("config/rh/gems.yml", "rails_dependent:\n  - {gem: widget-engine, why: requiring it loads 3 Rails framework files}\n"));
    }
    files
}


#[test]
fn the_ruby_target_refuses_a_rails_dependent_constant_with_the_evidence() {
    let transpile = |rails_dependent: bool| {
        let root = std::env::temp_dir().join(format!("roundhouse_rails_dependent_{rails_dependent}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in engine_files(rails_dependent) {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", "ruby", "-o"])
            .arg(root.join("out"))
            .arg(&root)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let (ok, stderr) = transpile(false);
    assert!(ok, "the CRuby tree carries an unknown locked gem: {stderr}");
    let (ok, stderr) = transpile(true);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("WidgetEngine::Client"), "{stderr}");
    assert!(
        stderr.contains("a project extension marks gem widget-engine rails_dependent: requiring it loads 3 Rails framework files"),
        "{stderr}"
    );
}

// ── Isolation ────────────────────────────────────────────────────────

/// One app per index, each with its own macro name and runtime gem.
fn isolated_app(i: usize) -> Vec<(String, String)> {
    vec![
        ("roundhouse.yml".into(), "extensions: [ext]\n".into()),
        ("ext/macros.yml".into(), format!("attribute_macros:\n  - {{name: price_column_{i}, positional: true}}\n")),
        ("ext/gems.yml".into(), format!("runtime_gems:\n  - {{constant: Widget{i}, gem: widget-{i}}}\n")),
        ("db/schema.rb".into(), INVOICE_SCHEMA.into()),
        ("app/models/application_record.rb".into(), APPLICATION_RECORD.into()),
        (
            "app/models/invoice.rb".into(),
            format!("class Invoice < ApplicationRecord\n  price_column_{i} :amount\n\n  def self.label\n    find_by(id: 1).amount.currency\n  end\n\n  def self.widget\n    Widget{i}.new\n  end\nend\n"),
        ),
    ]
}

fn ingest_isolated(i: usize) -> App {
    let files = isolated_app(i);
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    ingest(&borrowed).expect("ingest")
}

/// What app `i` must see of its own facts, and nothing of any other app's.
fn assert_sees_only_its_own(i: usize, count: usize, mut app: App) {
    let macros: Vec<&str> = app.extensions.attribute_macros.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(macros, [format!("price_column_{i}")]);
    Analyzer::new(&app).analyze(&mut app);
    assert!(dispatch_failures(&app).is_empty(), "app {i}: {:#?}", codes(&diagnose(&app)));
    let gemfile = file(&emitted(&app, BuildTarget::Ruby), "Gemfile").to_string();
    for other in 0..count {
        let line = format!("gem \"widget-{other}\"");
        assert_eq!(gemfile.contains(&line), other == i, "app {i} and {line}:\n{gemfile}");
    }
}

#[test]
fn an_app_keeps_its_facts_when_another_is_ingested_in_between() {
    // A process- or thread-wide table would hand app 0's analysis the facts
    // of whichever app was ingested last.
    let first = ingest_isolated(0);
    let bare = analyzed(&[
        ("db/schema.rb", INVOICE_SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/invoice.rb", "class Invoice < ApplicationRecord\n  price_column_0 :amount\n\n  def self.label\n    find_by(id: 1).amount.currency\n  end\nend\n"),
    ]);
    let second = ingest_isolated(1);
    assert_eq!(dispatch_failures(&bare).len(), 1, "an app without extensions does not see another app's macros");
    assert_sees_only_its_own(0, 2, first);
    assert_sees_only_its_own(1, 2, second);
}

#[test]
fn apps_with_different_extensions_run_side_by_side() {
    const COUNT: usize = 6;
    let start = Barrier::new(COUNT);
    let ingested = Barrier::new(COUNT);
    std::thread::scope(|scope| {
        for i in 0..COUNT {
            let (start, ingested) = (&start, &ingested);
            std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn_scoped(scope, move || {
                    start.wait();
                    // Caught so a failed ingest cannot strand the others at
                    // the barrier; it is re-raised once everyone is past it.
                    let app = std::panic::catch_unwind(|| ingest_isolated(i));
                    // Every app is ingested before any is analyzed or emitted.
                    ingested.wait();
                    assert_sees_only_its_own(i, COUNT, app.unwrap_or_else(|panic| std::panic::resume_unwind(panic)));
                })
                .unwrap();
        }
    });
}
