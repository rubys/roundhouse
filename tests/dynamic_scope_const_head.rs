//! PostgreSQL: dynamic scope const head, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BUILDER: &str = "class Builder\n  class Wrapper; end\nend\n";

const CFG: &str = "module Cfg\n  class << self\n    attr_accessor :builder\n  end\n\n  self.builder = Builder\nend\n";

const CHECK: &str = "class DynConst\n  def check(x)\n    x.is_a?(Cfg.builder::Wrapper)\n  end\nend\n";

#[test]
#[ignore = "fork spec: postgresql"]
fn the_constant_heading_a_value_scope_is_read_not_refused() {
    let app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("app/models/builder.rb", BUILDER)
        .write("app/models/cfg.rb", CFG)
        .write("app/models/dyn_const.rb", CHECK);
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("constant not supported")), "{}", errors.join("\n"));
    let check = std::fs::read_to_string(emitted.join("app/models/dyn_const.rb")).unwrap();
    assert!(check.contains("Cfg.builder.const_get(:Wrapper)"), "{check}");
    assert!(!check.contains("constant not supported"), "{check}");
}
