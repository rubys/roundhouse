//! `ActiveStorage::Current` analyzer surface matches
//! `runtime/ruby/active_storage.rbs`: `url_options`, `url_options=`,
//! `set`, and `base_url` must dispatch (not `SendDispatchFailed`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ident::{ClassId, Symbol};

fn analyze_controller(body: &str) -> roundhouse::App {
    let controller = format!("class BlobsController < ApplicationController\n  def show\n{body}\n  end\nend\n");
    let files: [(&str, &str); 3] = [
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/blobs_controller.rb", &controller),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn active_storage_current_registry_lists_rbs_surface() {
    let app = analyze_controller("    render plain: \"x\"\n");
    let analyzer = Analyzer::new(&app);
    let current = analyzer
        .class_registry()
        .get(&ClassId(Symbol::from("ActiveStorage::Current")))
        .expect("ActiveStorage::Current registered");
    for name in ["url_options", "url_options=", "set", "base_url"] {
        assert!(
            current.class_methods.contains_key(&Symbol::from(name)),
            "missing class method {name}; have {:?}",
            current
                .class_methods
                .keys()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn active_storage_current_accessors_dispatch_without_error() {
    let app = analyze_controller(
        r#"    opts = ActiveStorage::Current.url_options
    ActiveStorage::Current.url_options = { host: "example.test", port: 3000 }
    url = ActiveStorage::Current.base_url
    ActiveStorage::Current.set(url_options: { host: "x.test" }) { url }
    render plain: url.to_s + opts.to_s
"#,
    );
    let dispatch: Vec<_> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => Some(method.as_str().to_string()),
            _ => None,
        })
        .filter(|m| {
            matches!(
                m.as_str(),
                "url_options" | "url_options=" | "base_url" | "set"
            )
        })
        .collect();
    assert!(
        dispatch.is_empty(),
        "ActiveStorage::Current accessors must dispatch; failures = {dispatch:?}"
    );
}
