use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table :invoices do |t|\n    t.integer :total\n  end\nend\n";

fn lockfile(remote: &str) -> String {
    format!("PATH\n  remote: {remote}\n  specs:\n    billing (0.1.0)\n\n")
}

fn tree_app(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn library_only_path_gems_are_ingested_once_and_unlisted_gems_stay_out() {
    let lock = format!(
        "{}{}",
        lockfile("components/billing"),
        lockfile("./components/billing")
    );
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        (
            "components/billing/lib/billing.rb",
            "class Billing\n  def self.total\n    7\n  end\nend\n",
        ),
        ("packs/unlisted/lib/unlisted.rb", "class Unlisted\nend\n"),
    ]);
    let names: Vec<_> = app
        .library_classes
        .iter()
        .map(|class| class.name.0.as_str())
        .collect();
    assert_eq!(names, vec!["Billing"]);
    assert_eq!(app.app_roots, vec!["app"]);
}

#[test]
fn path_gem_bases_are_available_before_model_classification() {
    let lock = lockfile("packs/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        (
            "packs/billing/lib/billing_record.rb",
            "class BillingRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/invoice.rb",
            "class Invoice < BillingRecord\nend\n",
        ),
    ]);
    assert!(
        app.models
            .iter()
            .any(|model| model.name.0.as_str() == "Invoice")
    );
    assert!(
        !app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "Invoice")
    );
}

#[test]
fn path_sources_outside_the_app_do_not_add_library_classes() {
    for remote in ["../billing", "/srv/billing"] {
        let lock = lockfile(remote);
        let app = tree_app(&[
            ("Gemfile.lock", &lock),
            ("components/billing/lib/billing.rb", "class Billing\nend\n"),
        ]);
        assert!(
            !app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == "Billing")
        );
    }
}

#[cfg(unix)]
mod on_disk {
    use super::*;
    use roundhouse::ingest::ingest_app;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roundhouse_{name}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create temporary root");
        root.canonicalize().expect("canonicalize temporary root")
    }

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (relative, source) in files {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("create parent");
            std::fs::write(path, source).expect("write source");
        }
    }

    #[test]
    fn engine_app_and_lib_walks_exclude_linked_files_and_directories() {
        let root = temp_root("path_gem_descendant_links");
        let host = root.join("host");
        let lock = lockfile("components/billing");
        write(
            &host,
            &[
                ("Gemfile.lock", &lock),
                ("db/schema.rb", SCHEMA),
                ("app/models/application_record.rb", APPLICATION_RECORD),
                (
                    "components/billing/lib/billing/engine.rb",
                    "module Billing\n  class Engine < Rails::Engine\n  end\nend\n",
                ),
                ("components/billing/lib/local.rb", "class Local\nend\n"),
                (
                    "components/billing/app/models/invoice.rb",
                    "class Invoice < ApplicationRecord\nend\n",
                ),
            ],
        );
        write(
            &root,
            &[
                (
                    "outside/model.rb",
                    "class OutsideModel < ApplicationRecord\nend\n",
                ),
                ("outside/library.rb", "class OutsideLibrary\nend\n"),
                ("outside/views/index.html.erb", "<p>outside</p>\n"),
                ("outside/lib/foreign.rb", "class Foreign\nend\n"),
            ],
        );
        let gem = host.join("components/billing");
        std::fs::create_dir_all(gem.join("app/views")).expect("create views");
        for (target, link) in [
            (
                root.join("outside/model.rb"),
                gem.join("app/models/outside.rb"),
            ),
            (root.join("outside/library.rb"), gem.join("lib/outside.rb")),
            (root.join("outside/views"), gem.join("app/views/outside")),
            (root.join("outside/lib"), gem.join("lib/foreign")),
            (gem.join("lib"), gem.join("lib/cycle")),
        ] {
            std::os::unix::fs::symlink(target, link).expect("create descendant link");
        }

        let app = ingest_app(&host).expect("ingest engine");
        assert_eq!(app.app_roots, vec!["app", "components/billing/app"]);
        assert!(
            app.models
                .iter()
                .any(|model| model.name.0.as_str() == "Invoice")
        );
        assert!(
            !app.models
                .iter()
                .any(|model| model.name.0.as_str() == "OutsideModel")
        );
        let names: Vec<_> = app
            .library_classes
            .iter()
            .map(|class| class.name.0.as_str())
            .collect();
        assert!(names.contains(&"Local"));
        assert!(!names.contains(&"OutsideLibrary"));
        assert!(!names.contains(&"Foreign"));
        assert!(
            !app.views
                .iter()
                .any(|view| view.name.as_str().starts_with("outside/"))
        );
        assert!(
            !app.sources
                .iter()
                .any(|source| source.path.contains("/outside") || source.path.contains("/foreign"))
        );
        std::fs::remove_dir_all(root).expect("remove temporary tree");
    }

    #[test]
    fn linked_gem_lib_does_not_supply_an_engine_declaration() {
        let root = temp_root("path_gem_linked_lib");
        let host = root.join("host");
        let lock = lockfile("components/billing");
        write(
            &host,
            &[
                ("Gemfile.lock", &lock),
                ("db/schema.rb", SCHEMA),
                ("app/models/application_record.rb", APPLICATION_RECORD),
                (
                    "components/billing/app/models/invoice.rb",
                    "class Invoice < ApplicationRecord\nend\n",
                ),
            ],
        );
        write(
            &root,
            &[("outside/engine.rb", "class Engine < Rails::Engine\nend\n")],
        );
        std::os::unix::fs::symlink(root.join("outside"), host.join("components/billing/lib"))
            .expect("link gem lib");
        let app = ingest_app(&host).expect("ingest host");
        assert_eq!(app.app_roots, vec!["app"]);
        assert!(
            !app.models
                .iter()
                .any(|model| model.name.0.as_str() == "Invoice")
        );
        assert!(
            !app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == "Engine")
        );
        std::fs::remove_dir_all(root).expect("remove temporary tree");
    }
}
