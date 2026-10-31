//! PostgreSQL: load hook included concern, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const HELPER: &str = r#"module Probe
  module Helper
    extend(ActiveSupport::Concern)

    module ClassMethods
      def probe_macro(label)
        (@probes ||= []) << label
      end

      def probes
        @probes
      end
    end

    def instance_probe
      "instance"
    end
  end
end
"#;

/// money-kit's railtie shape: the hook registered in a railtie body.
const RAILTIE: &str = r#"module Probe
  class Railtie < Rails::Railtie
    ActiveSupport.on_load(:probe_hook) do
      include Probe::Helper
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concern_included_by_a_load_hook_extends_the_base_with_its_class_methods() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_load_hook_concern\n")
        .write("lib/oracle.rb", "class Oracle\nend\n")
        .write("lib/probe/helper.rb", HELPER)
        .write("lib/probe/railtie.rb", RAILTIE)
        .write("config/application.rb", "require \"rails\"\n\nmodule ProbeApp\n  class Application < Rails::Application\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    let helper = std::fs::read_to_string(dir.join("app/models/probe/helper.rb")).expect("helper.rb");
    assert!(helper.contains("extend(ActiveSupport::Concern)"), "{helper}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            "require_relative 'runtime/active_support_lazy_load_hooks'\n\
             require_relative 'runtime/active_support_concern'\n\
             require_relative 'app/models/probe/helper'\n\
             require_relative 'app/models/probe/helper/class_methods'\n\
             class ProbeBase; end\n\
             ActiveSupport.on_load(:probe_hook) { include Probe::Helper }\n\
             ActiveSupport.run_load_hooks(:probe_hook, ProbeBase)\n\
             ProbeBase.probe_macro(:one)\n\
             p [ProbeBase.probes, ProbeBase.new.instance_probe, ProbeBase.singleton_class.include?(Probe::Helper::ClassMethods)]\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "[[:one], \"instance\", true]", "{stderr}");
}
