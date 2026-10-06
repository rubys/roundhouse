//! Circular module accessors must not thrash the harvest table.
//!
//! A `config` / `Configuration.load!` cycle can alternate
//! `Configuration ↔ Configuration|Untyped` every fixpoint round and
//! exhaust `FIXPOINT_CAP`. Stabilizing returns that differ only by
//! top-level `Untyped`/`Var` arms lets the signature converge.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

const CIRCULAR: &str = r#"
module Probe
  def self.config
    @config ||= Configuration.load!
  end

  class Configuration
    def self.load!
      if config_exists?
        Probe.config.apply_upgrades
      else
        new
      end
    end

    def self.config_exists?
      true
    end

    def apply_upgrades
      self
    end
  end
end

class Client
  def boot
    Probe.config.apply_upgrades
  end
end
"#;

#[test]
fn circular_config_load_emits_configuration_not_gradual_churn() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/probe.rb"), CIRCULAR.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    // `config` / `load!` must settle on Configuration (possibly nullable),
    // not remain stuck as a thrashing Configuration|untyped harvest.
    assert!(
        rbs.contains("Configuration") && !rbs.contains("Configuration | untyped"),
        "expected a settled Configuration return, got:\n{rbs}"
    );
}
