//! `ingest::app::app_roots` — a Rails engine kept in the app's own
//! tree (`gem "billing", path: "lib/billing"`) contributes its `app/`
//! as an app-layer root, the way Rails adds an engine's `app/*` to the
//! host's autoload and view paths. See `engine_app_roots` in
//! `src/ingest/app.rs`; this pins discovery from `Gemfile.lock`'s
//! `PATH` sources and the shapes that must NOT become a root: a path
//! gem with no engine class, one outside the app's tree, one whose
//! `app/` is a symbolic link, and an engine-shaped directory the
//! lockfile does not name.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
  create_table "invoices", force: :cascade do |t|
    t.integer "total"
  end
end
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const ARTICLE_MODEL: &str = "class Article < ApplicationRecord\nend\n";
const INVOICE_MODEL: &str = "class Invoice < ApplicationRecord\nend\n";
const INVOICES_CONTROLLER: &str =
    "class InvoicesController < ApplicationController\n  def index\n    @invoices = Invoice.all\n  end\nend\n";
const INVOICES_INDEX_VIEW: &str = "<%= @invoices.length %>\n";
const INVOICE_TOTALS: &str = "class InvoiceTotals\n  def self.sum(invoices)\n    invoices\n  end\nend\n";
const ENGINE: &str = "module Billing\n  class Engine < ::Rails::Engine\n    isolate_namespace Billing\n  end\nend\n";

/// Construct the lockfile metadata that selects one local engine.
fn lockfile(remote: &str) -> String {
    format!(
        "PATH\n  remote: {remote}\n  specs:\n    billing (0.1.0)\n      rails\n\n\
         GEM\n  remote: https://rubygems.org/\n  specs:\n    rails (8.0.2)\n\n\
         DEPENDENCIES\n  billing!\n  rails\n"
    )
}

/// Exercise the same ingest entry point as the in-memory application frontend.
fn tree_app(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

/// Collect model identities so the tests detect classification and duplicate errors.
fn model_names(app: &roundhouse::App) -> Vec<&str> {
    app.models.iter().map(|m| m.name.0.as_str()).collect()
}

/// (a) A path-sourced engine's `app/` is walked exactly like the
/// root's: models, controllers, views, and every other layer.
#[test]
fn path_engine_app_is_an_app_root() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/models/invoice.rb", INVOICE_MODEL),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ("lib/billing/app/views/invoices/index.html.erb", INVOICES_INDEX_VIEW),
        ("lib/billing/app/services/invoice_totals.rb", INVOICE_TOTALS),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "lib/billing/app".to_string()]);
    assert!(model_names(&app).contains(&"Article"), "root model kept: {:?}", model_names(&app));
    // Exactly once: the root `lib/` walk reaches the engine's tree
    // too, and must leave it to the engine's own root.
    assert_eq!(
        model_names(&app).iter().filter(|name| **name == "Invoice").count(),
        1,
        "lib/billing/app/models/invoice.rb should be one model: {:?}",
        model_names(&app)
    );
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"),
        "lib/billing/app/controllers/invoices_controller.rb should be a controller"
    );
    let library_names: Vec<&str> = app.library_classes.iter().map(|c| c.name.0.as_str()).collect();
    for name in ["InvoicesController", "InvoiceTotals", "Billing::Engine"] {
        assert!(
            library_names.iter().filter(|n| **n == name).count() <= 1,
            "{name} should not be ingested twice: {library_names:?}"
        );
    }
    assert!(
        !library_names.contains(&"InvoicesController"),
        "an engine controller is a controller, not a library class: {library_names:?}"
    );
    assert!(
        app.views.iter().any(|v| v.name.as_str() == "invoices/index"),
        "lib/billing/app/views/invoices/index.html.erb should address as invoices/index: {:?}",
        app.views.iter().map(|v| v.name.as_str().to_string()).collect::<Vec<_>>()
    );
    assert!(
        app.library_classes.iter().any(|c| c.name.0.as_str() == "InvoiceTotals"),
        "lib/billing/app/services/invoice_totals.rb should register InvoiceTotals: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}

/// (b) A path gem with an `app/` directory but no `Rails::Engine`
/// subclass is a plain library: Rails never loads its `app/`.
#[test]
fn path_gem_without_an_engine_class_is_not_a_root() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("lib/billing/lib/billing.rb", "module Billing\nend\n"),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string()]);
    assert!(!app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
}

/// (c) An engine-shaped directory the lockfile does not name is not
/// loaded by the app — the behavior before this change.
#[test]
fn engine_not_in_the_lockfile_leaves_behavior_unchanged() {
    let app = tree_app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string()]);
    assert!(!app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
}

/// (d) A path source outside the app's tree is someone else's source,
/// and `remote: .` (an app that is itself a gem) is the root `app`
/// already.
#[test]
fn path_sources_outside_the_tree_or_at_its_root_add_nothing() {
    for remote in ["../billing", "/srv/billing", "."] {
        let lock = lockfile(remote);
        let app = tree_app(&[
            ("Gemfile.lock", &lock),
            ("db/schema.rb", SCHEMA),
            ("app/models/application_record.rb", APPLICATION_RECORD),
            ("app/models/article.rb", ARTICLE_MODEL),
            ("lib/billing/engine.rb", ENGINE),
        ]);
        assert_eq!(app.app_roots, vec!["app".to_string()], "remote: {remote}");
    }
}

/// (e) `Rails::Engine` is matched as a whole constant: a class named
/// with that prefix is not an engine.
#[test]
fn a_superclass_that_only_starts_with_rails_engine_is_not_an_engine() {
    // The last one names its superclass only in a comment.
    for header in [
        "class Engine < Rails::EngineStub",
        "class Engine < Rails::Engine::Configuration",
        "class Engine # < Rails::Engine",
    ] {
        let lock = lockfile("lib/billing");
        let source = format!("module Billing\n  {header}\n  end\nend\n");
        let app = tree_app(&[
            ("Gemfile.lock", &lock),
            ("db/schema.rb", SCHEMA),
            ("app/models/application_record.rb", APPLICATION_RECORD),
            ("lib/billing/lib/billing/engine.rb", &source),
            ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ]);
        assert_eq!(app.app_roots, vec!["app".to_string()], "{header}");
    }
}

#[test]
fn a_multiline_engine_superclass_is_an_app_root() {
    let lock = lockfile("components/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("components/billing/lib/billing/engine.rb", "module Billing\n  class Engine <\n    ::Rails::Engine\n  end\nend\n"),
        ("components/billing/app/models/invoice.rb", INVOICE_MODEL),
    ]);
    assert_eq!(app.app_roots, vec!["app", "components/billing/app"]);
    assert!(model_names(&app).contains(&"Invoice"));
}

#[test]
fn engine_declarations_inside_a_heredoc_do_not_add_roots() {
    let lock = lockfile("components/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("components/billing/lib/example.rb", "module Billing\n  EXAMPLE = <<~RUBY\n    class Engine < Rails::Engine\n    end\n  RUBY\nend\n"),
        ("components/billing/app/models/invoice.rb", INVOICE_MODEL),
    ]);
    assert_eq!(app.app_roots, vec!["app"]);
    assert!(!model_names(&app).contains(&"Invoice"));
}

/// (f) A Ruby file directly in the engine's `app/` has no layer pass
/// of its own, so the root `lib/` walk that reaches it keeps it.
#[test]
fn a_file_directly_in_the_engine_app_is_still_ingested_once() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/entry.rb", "class Entry\n  def self.call\n    1\n  end\nend\n"),
        ("lib/billing/app/services/invoice_totals.rb", INVOICE_TOTALS),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "lib/billing/app".to_string()]);
    let library_names: Vec<&str> = app.library_classes.iter().map(|c| c.name.0.as_str()).collect();
    for name in ["Entry", "InvoiceTotals"] {
        assert_eq!(
            library_names.iter().filter(|n| **n == name).count(),
            1,
            "{name} should be ingested exactly once: {library_names:?}"
        );
    }
}

/// (i) The app's template shadows an engine's of the same name and
/// format, as Rails' view paths resolve it: the app's own `app/views`
/// comes first. Another format of the same name is a different
/// template and is kept.
#[test]
fn an_app_template_shadows_the_engines_copy() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/views/invoices/index.html.erb", "<p>HOST COPY</p>\n"),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/models/invoice.rb", INVOICE_MODEL),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ("lib/billing/app/views/invoices/index.html.erb", "<p>ENGINE COPY</p>\n"),
        ("lib/billing/app/views/invoices/index.text.erb", "ENGINE TEXT\n"),
        ("lib/billing/app/views/invoices/show.html.erb", "<p>ENGINE SHOW</p>\n"),
    ]);

    let rendered = |name: &str, format: &str| -> Vec<String> {
        app.views
            .iter()
            .filter(|v| v.name.as_str() == name && v.format.as_str() == format)
            .map(|v| format!("{:?}", v.body))
            .collect()
    };
    let index = rendered("invoices/index", "html");
    assert_eq!(index.len(), 1, "one invoices/index.html, not two");
    assert!(index[0].contains("HOST COPY") && !index[0].contains("ENGINE COPY"), "{index:?}");
    // Not shadowed: a format the app does not override, and a template
    // only the engine has.
    assert_eq!(rendered("invoices/index", "text").len(), 1);
    assert_eq!(rendered("invoices/show", "html").len(), 1);
}

#[cfg(unix)]
mod on_disk {
    use super::*;
    use roundhouse::ingest::ingest_app;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Create an isolated application root without host-system path aliases.
    fn unique_tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "roundhouse_{name}_{}_{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // `/var` → `/private/var` on macOS: the app's own path must
        // not itself contain a link.
        dir.canonicalize().expect("canonicalize temp dir")
    }

    /// Populate one application tree with the source files that the case needs.
    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, content) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).expect("create parent");
            std::fs::write(path, content).expect("write file");
        }
    }

    /// (g) An engine whose `app/` is a symbolic link is not a root:
    /// following it would walk a tree outside the app.
    #[test]
    fn engine_with_a_linked_app_directory_is_not_a_root() {
        let root = unique_tmp_dir("engine_linked_app");
        let app_root = root.join("host");
        let lock = lockfile("lib/billing");
        write(&app_root, &[
            ("Gemfile.lock", &lock),
            ("db/schema.rb", SCHEMA),
            ("app/models/application_record.rb", APPLICATION_RECORD),
            ("lib/billing/lib/billing/engine.rb", ENGINE),
        ]);
        write(&root, &[("outside/controllers/invoices_controller.rb", INVOICES_CONTROLLER)]);
        std::os::unix::fs::symlink(root.join("outside"), app_root.join("lib/billing/app"))
            .expect("link the engine app directory");

        let app = ingest_app(&app_root).expect("ingest app");
        assert_eq!(app.app_roots, vec!["app".to_string()]);
        assert!(!app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
        std::fs::remove_dir_all(root).expect("remove temp app");
    }

    /// (h) An absolute `remote:` that names a directory inside the app
    /// is the same engine as its relative spelling.
    #[test]
    fn absolute_remote_inside_the_app_is_a_root() {
        let root = unique_tmp_dir("engine_absolute_remote");
        let app_root = root.join("host");
        let lock = lockfile(&app_root.join("lib/billing").display().to_string());
        write(&app_root, &[
            ("Gemfile.lock", &lock),
            ("db/schema.rb", SCHEMA),
            ("app/models/application_record.rb", APPLICATION_RECORD),
            ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
            ("lib/billing/lib/billing/engine.rb", ENGINE),
            ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ]);

        let app = ingest_app(&app_root).expect("ingest app");
        assert_eq!(app.app_roots, vec!["app".to_string(), "lib/billing/app".to_string()]);
        assert!(app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
        std::fs::remove_dir_all(root).expect("remove temp app");
    }

    #[test]
    fn absolute_remote_is_independent_of_the_app_argument_spelling() {
        let root = PathBuf::from("target").join(format!(
            "engine_relative_root_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let host = root.join("host");
        std::fs::create_dir_all(&host).expect("create host");
        let absolute_host = host.canonicalize().expect("absolute host");
        let lock = format!(
            "{}{}",
            lockfile(&absolute_host.join("components/billing").display().to_string()),
            lockfile(&absolute_host.parent().unwrap().join("outside").display().to_string())
        );
        write(&host, &[
            ("Gemfile.lock", &lock),
            ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
            ("components/billing/lib/billing/engine.rb", ENGINE),
            ("components/billing/lib/local.rb", "class Local\nend\n"),
            ("components/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ]);
        write(&root, &[("outside/lib/foreign.rb", "class Foreign\nend\n")]);

        let app = ingest_app(&host).expect("ingest relative host");
        assert_eq!(app.app_roots, vec!["app", "components/billing/app"]);
        assert!(app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
        assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Local"));
        assert!(!app.library_classes.iter().any(|c| c.name.0.as_str() == "Foreign"));
        std::fs::remove_dir_all(root).expect("remove temp app");
    }
}
