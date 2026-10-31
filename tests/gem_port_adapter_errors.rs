//! PostgreSQL: gem port adapter errors, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::path::{Path, PathBuf};

use roundhouse::project::BuildTarget;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

const ERRORS: &str = "module AdapterErrorProbe\n  TRANSIENT = [\n    ActiveRecord::ConnectionAdapters::TrilogyAdapter::ResourceBusyError,\n    ActiveRecord::ConnectionAdapters::TrilogyAdapter::CircuitOpenError,\n    ::ActiveRecord::ConnectionAdapters::TrilogyAdapter::ResourceBusyError,\n  ].freeze\n  def self.trilogy?(connection)\n    defined?(ActiveRecord::ConnectionAdapters::TrilogyAdapter) && connection.is_a?(ActiveRecord::ConnectionAdapters::TrilogyAdapter)\n  end\nend\n";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

fn app(postgres: bool) -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "rh-gem-port-adapter-errors-{}-{}-{:?}",
        postgres,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    copy_tree(&Path::new(MANIFEST_DIR).join("fixtures/tiny-blog"), &dir);
    let write = |rel: &str, text: &str| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("app/models/adapter_error_probe.rb", ERRORS);
    if postgres {
        write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n");
        write("roundhouse.yml", "");
        write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_spec_gem_port_adapter_errors\n");
    }
    TempDir(dir)
}

fn probe(app: &TempDir) -> (String, Vec<String>) {
    let mut ingested = roundhouse::ingest::ingest_app(&app.0).expect("ingest fixture");
    let lowered = roundhouse::session::analyze_and_lower(&mut ingested);
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&ingested, &Path::new(MANIFEST_DIR).join("fixtures/tiny-blog"), BuildTarget::Ruby)
    });
    let messages = roundhouse::analyze::diagnose(&ingested).into_iter().chain(lowered).chain(diags)
        .map(|d| d.message).collect();
    let text = files.expect("package the Ruby project").into_iter()
        .find(|(p, _)| p == "app/models/adapter_error_probe.rb")
        .map(|(_, text)| text).expect("emitted adapter_error_probe.rb");
    (text, messages)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn adapter_error_constants_a_port_declares_resolve_on_the_tree_that_carries_the_port() {
    let (text, messages) = probe(&app(true));
    let refused: Vec<_> = messages.iter().filter(|m| m.contains("TrilogyAdapter")).collect();
    assert!(refused.is_empty(), "the read was refused: {refused:#?}");
    assert!(!text.contains("roundhouse:"), "{text}");
    assert!(text.contains("TrilogyAdapter::ResourceBusyError") && text.contains("TrilogyAdapter::CircuitOpenError"), "{text}");
}
