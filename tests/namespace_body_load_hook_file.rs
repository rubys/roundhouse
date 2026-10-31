//! PostgreSQL: namespace body load hook file, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

const REGISTRY: &str = "module Optimization\n  module Rollouts\n    module ChangesetRegistry\n      def self.entries\n        @entries ||= []\n      end\n    end\n\n    ActiveSupport.run_load_hooks(:rollout_changesets_registry, Optimization::Rollouts)\n\n    LIMIT = 2\n\n    if ChangesetRegistry.entries.size < LIMIT\n      ChangesetRegistry.entries << $fired.inspect\n    end\n  end\nend\n";
const ABSTRACT: &str = "module Optimization\n  module Rollouts\n    module AbstractChangeset\n      def self.kind\n        \"abstract\"\n      end\n    end\n  end\nend\n";
/// Registers a counter on the hook, loads the sibling and then the
/// registry (`LOAD`: how a file is loaded), printing the count after each.
const SCRIPT: &str = "ActiveSupport.on_load(:rollout_changesets_registry) { $fired = ($fired || 0) + 1 }\nLOAD.call(\"abstract_changeset\")\nputs Optimization::Rollouts::AbstractChangeset.kind\nputs $fired.inspect\nLOAD.call(\"changeset_registry\")\nputs $fired.inspect\nputs Optimization::Rollouts::ChangesetRegistry.entries.inspect\n";

/// The namespace statement below the call (`LIMIT` gives the namespace a
/// body of its own) runs after the hook, as it is written.
const EXPECTED: [&str; 4] = ["abstract", "nil", "1", "[\"1\"]"];

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_namespace_bodys_load_hook_fires_with_the_file_that_wrote_it() {
    let root = std::env::temp_dir().join(format!("roundhouse-namespace-load-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("changeset_registry.rb"), REGISTRY).unwrap();
    std::fs::write(root.join("abstract_changeset.rb"), ABSTRACT).unwrap();
    // CRuby on Rails' own lazy_load_hooks: the sibling's load fires
    // nothing, the registry's fires the hook once.
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"active_support\"\nrequire \"active_support/lazy_load_hooks\"\nLOAD = ->(f) {{ require File.join({:?}, f) }}\n{SCRIPT}", root.display().to_string()))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(lines(&native.stdout), EXPECTED, "native control");

    let (tree, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_namespace_body_load_hook\n")
        .write("app/models/optimization/rollouts/changeset_registry.rb", REGISTRY)
        .write("app/models/optimization/rollouts/abstract_changeset.rb", ABSTRACT)
        .emit(BuildTarget::Ruby);
    let read = |rel: &str| std::fs::read_to_string(tree.join(rel)).unwrap_or_default();
    let registry = read("app/models/optimization/rollouts/changeset_registry.rb");
    assert!(
        registry.contains("ActiveSupport.run_load_hooks(:rollout_changesets_registry, Optimization::Rollouts)"),
        "the hook does not end the file that wrote it:\n{registry}\nerrors: {errors:#?}"
    );
    let namespace = read("app/models/optimization/rollouts.rb");
    assert!(!namespace.contains("run_load_hooks"), "the namespace's file fires the hook:\n{namespace}");
    let hook = registry.find("ActiveSupport.run_load_hooks(").unwrap_or_default();
    let statement = registry.find("ChangesetRegistry.entries << $fired.inspect").unwrap_or_else(|| panic!("the namespace statement is not in its file:\n{registry}\nrollouts.rb:\n{namespace}"));
    assert!(hook < statement, "the statement below the hook runs before it:\n{registry}");

    // The PostgreSQL tree cannot boot without the `pg` gem, so the
    // emitted files load on CRuby in the order an autoload of each
    // constant loads them, on the runtime's lazy_load_hooks.
    let emitted = Command::new("ruby")
        .current_dir(&tree)
        .arg("-e")
        .arg(format!(
            "require_relative File.join(Dir.pwd, \"runtime/active_support_lazy_load_hooks\")\n\
             LOAD = ->(f) {{ require File.join(Dir.pwd, \"app/models/optimization/rollouts\") if File.exist?(File.join(Dir.pwd, \"app/models/optimization/rollouts.rb\")); require File.join(Dir.pwd, \"app/models/optimization/rollouts\", f) }}\n{SCRIPT}"
        ))
        .output()
        .expect("spawn ruby");
    let stderr = String::from_utf8_lossy(&emitted.stderr);
    assert!(emitted.status.success(), "emitted files failed in {}: {stderr}\nerrors: {errors:#?}", tree.display());
    assert_eq!(lines(&emitted.stdout), EXPECTED, "{stderr}\nerrors: {errors:#?}");
    let _ = std::fs::remove_dir_all(&root);
}
