//! PostgreSQL: namespace body in model namespace, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const TAXONOMY: &str = "module Merchandising\n  class Taxonomy < ApplicationRecord\n  end\nend\n";
const REGISTRY_HOOK: &str = "module Merchandising\n  class Taxonomy\n    module Owner\n      module Registry\n        def self.owners\n          @owners ||= []\n        end\n      end\n\n      ActiveSupport.run_load_hooks(:taxonomy_owners_registry, Merchandising)\n    end\n  end\nend\n";
const REGISTRY_STATEMENT: &str = "module Merchandising\n  class Taxonomy\n    module Owner\n      module Registry\n        def self.owners\n          @owners ||= []\n        end\n      end\n\n      LIMIT = 3\n\n      if Registry.owners.size < LIMIT\n        Registry.owners << \"checked\"\n      end\n    end\n  end\nend\n";
const APP: &str = "module Merchandising\n  class Taxonomy\n    module Owner\n      class App\n        def self.display_name\n          \"app\"\n        end\n      end\n    end\n  end\nend\n";

fn emit(registry: &str) -> (emit_and_run::Emitted, Vec<String>) {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_namespace_body_in_model\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/merchandising/taxonomy.rb", TAXONOMY)
        .write("app/models/merchandising/taxonomy/owner/registry.rb", registry)
        .write("app/models/merchandising/taxonomy/owner/app.rb", APP)
        .emit(BuildTarget::Ruby)
}

/// Every `require_relative` under app/models names a file of the tree.
fn unresolved_requires(tree: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![tree.join("app/models")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for line in text.lines() {
                let Some(rest) = line.trim_start().strip_prefix("require_relative \"") else { continue };
                let target = rest.split('"').next().unwrap_or_default();
                let resolved = path.parent().unwrap().join(format!("{target}.rb"));
                if !resolved.exists() {
                    out.push(format!("{}: {line}", path.strip_prefix(tree).unwrap().display()));
                }
            }
        }
    }
    out
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_hook_after_a_declaration_in_a_models_namespace_writes_no_second_reopen() {
    let (tree, errors) = emit(REGISTRY_HOOK);
    let read = |rel: &str| std::fs::read_to_string(tree.join(rel)).unwrap_or_default();
    let owner = read("app/models/merchandising/taxonomy/owner.rb");
    assert!(owner.matches("module Owner").count() <= 1, "owner.rb reopens the namespace twice:\n{owner}\nerrors: {errors:#?}");
    let registry = read("app/models/merchandising/taxonomy/owner/registry.rb");
    assert!(registry.contains("ActiveSupport.run_load_hooks(:taxonomy_owners_registry, Merchandising)"), "registry.rb lacks its hook:\n{registry}");
    assert_eq!(unresolved_requires(&tree), Vec::<String>::new(), "owner.rb:\n{owner}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_statement_after_a_declaration_in_a_models_namespace_goes_after_it() {
    let (tree, errors) = emit(REGISTRY_STATEMENT);
    let read = |rel: &str| std::fs::read_to_string(tree.join(rel)).unwrap_or_default();
    let owner = read("app/models/merchandising/taxonomy/owner.rb");
    assert!(!owner.contains("Registry.owners << \"checked\""), "the namespace's file runs the registry file's statement:\n{owner}\nerrors: {errors:#?}");
    let registry = read("app/models/merchandising/taxonomy/owner/registry.rb");
    let declared = registry.find("def self.owners").unwrap_or_else(|| panic!("registry.rb lacks its module:\n{registry}"));
    let statement = registry.find("Registry.owners << \"checked\"").unwrap_or_else(|| panic!("registry.rb lacks the statement:\n{registry}\nowner.rb:\n{owner}"));
    assert!(declared < statement, "the statement precedes the declaration it follows:\n{registry}");
    assert_eq!(registry.matches("class Taxonomy").count(), 1, "registry.rb reopens the model twice:\n{registry}");
    assert_eq!(unresolved_requires(&tree), Vec::<String>::new(), "owner.rb:\n{owner}\nregistry.rb:\n{registry}");
}
