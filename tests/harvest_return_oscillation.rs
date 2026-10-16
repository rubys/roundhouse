//! Fixpoint rounds must not thrash the harvest table — and must not
//! launder a genuinely gradual return into a concrete one either.
//!
//! A `config` / `Configuration.load!` cycle can alternate
//! `Configuration ↔ Configuration|Untyped` every fixpoint round and
//! exhaust `FIXPOINT_CAP`. Stabilizing returns that differ only by
//! top-level `Untyped`/`Var` arms lets the signature converge.
//!
//! A different shape hits the same stabilizer: a parameter unbound on
//! the first round (a pending `Var`) and seeded `untyped` by a later
//! round's call site, with a concrete arm (`Sym`) present both times.
//! That `untyped` is not fixpoint noise — it is the only honest answer
//! — so it must survive, not be dropped for the concrete arm alone.

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

// `locale_for`'s parameter has no call site on the first pass over
// `lib/locale.rb` (`resolve` is defined after it), so round one types
// `x` from an unbound local — a pending `Var` — and `x&.dig("lang")`
// joins it with `Sym`. Once `resolve`'s body is walked, its call site
// seeds `x` as `untyped` (from `JSON.parse`'s unknown return), and
// round two's body type is `Sym | Untyped`. Both rounds share the
// `Sym` core, so stabilize used to drop the `untyped` arm outright —
// declaring `locale_for` `-> Symbol`, though it answers whatever
// `JSON.parse` read at the key. #964; #395 is the same family.
const LOCALE_PARAM: &str = r#"
module Locale
  def self.locale_for(x)
    x&.dig("lang") || :en
  end

  def self.resolve(raw)
    locale_for(JSON.parse(raw))
  end
end
"#;

#[test]
fn param_seeded_untyped_after_a_pending_var_keeps_the_gradual_arm() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/locale.rb"), LOCALE_PARAM.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rbs.contains("def self.locale_for: (untyped x) -> untyped"),
        "expected locale_for to stay declared -> untyped (it answers a JSON value, \
         not always a Symbol), got:\n{rbs}"
    );
}

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
    // RBS collapses `T | untyped` to bare `untyped`, so checking for the
    // substring "Configuration | untyped" never fails. Pin the settled
    // signatures themselves — without stabilize, `config` ends as
    // `-> untyped` while the class name still matches a naive search.
    assert!(
        rbs.contains("def self.config: () -> Probe::Configuration"),
        "expected Probe.config to settle on Probe::Configuration, got:\n{rbs}"
    );
    assert!(
        rbs.contains("def self.load!: () -> Probe::Configuration"),
        "expected Configuration.load! to settle on Probe::Configuration, got:\n{rbs}"
    );
}

