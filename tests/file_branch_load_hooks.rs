//! PostgreSQL: file branch load hooks, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

/// A branch holding only the registration: carried as the statement.
const DIGESTY: &str = r##"module Digesty
  def digesty = :yes
end

if defined? ActiveSupport
  ActiveSupport.on_load(:active_record) do
    include Digesty
  end
end
"##;

/// Branches holding declarations: carried as a branch item, and only
/// the branch Ruby runs registers its hook.
const STAMPED: &str = r##"if defined?(ActiveSupport)
  module Stamped
    def stamped = :stamped
  end

  ActiveSupport.on_load(:active_record) do
    include Stamped
  end
else
  module Unstamped
  end

  ActiveSupport.on_load(:active_record) do
    include Unstamped
  end
end
"##;

const SCRIPT: &str = r##"
p ActiveRecord::Base.include?(Digesty), ActiveRecord::Base.include?(Stamped), defined?(Unstamped)
p ActiveRecord::Base.instance_method(:digesty).owner.name, ActiveRecord::Base.instance_method(:stamped).owner.name
"##;

const EXPECTED: &str = "true\ntrue\nnil\n\"Digesty\"\n\"Stamped\"\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("lib/ext/digesty.rb", DIGESTY).write("lib/ext/stamped.rb", STAMPED)
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_hook_in_a_file_level_branch_registers_where_the_branch_runs() {
    // Registered as the files load, run when Active Record's base loads.
    let native = Command::new("ruby")
        .arg("-W0")
        .arg("-e")
        .arg(format!("require \"active_record\"\n{DIGESTY}\n{STAMPED}\nActiveRecord::Base\n{SCRIPT}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED);

    let (dir, errors) = app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_file_branch_load_hooks\n")
        .emit(BuildTarget::Ruby);
    let errors: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let stamped = read("app/models/roundhouse_files/lib/ext/stamped.rb");
    assert_eq!(stamped.matches("ActiveSupport.on_load(:active_record)").count(), 2, "{stamped}");
    // No initializer fires the hooks here: boot does, for the files'
    // registrations.
    let boot = read("boot.rb");
    let fire = boot
        .lines()
        .find(|line| line.starts_with("ActiveSupport.run_load_hooks(:active_record, ActiveRecord::Base)"))
        .unwrap_or_else(|| panic!("boot fires no :active_record hook:\n{boot}"));
    // The PostgreSQL tree cannot boot without the `pg` gem, so the
    // emitted pieces run in their load order on CRuby: the runtime's
    // lazy_load_hooks and Active Record, the emitted autoloader's
    // file-program helpers (app/models.rb's RoundhouseAutoload, without
    // its boot), the two file programs, and boot's hook firing.
    let models = read("app/models.rb");
    let autoload_end = models.find("\nend\n").expect("RoundhouseAutoload module") + "\nend\n".len();
    std::fs::write(dir.join("roundhouse_autoload_only.rb"), format!("{}\n", &models[..autoload_end]).replace("__dir__", "File.join(Dir.pwd, \"app\")"))
        .unwrap();
    let emitted_script = format!(
        "%w[runtime/active_support_lazy_load_hooks runtime/active_record roundhouse_autoload_only \
         app/models/roundhouse_files/lib/ext/digesty app/models/roundhouse_files/lib/ext/stamped]\
         .each {{ |f| require_relative File.join(Dir.pwd, f) }}\n{fire}\n{SCRIPT}"
    );
    let emitted = Command::new("ruby").arg("-W0").arg("-e").arg(&emitted_script).current_dir(&*dir).output().expect("ruby");
    assert!(emitted.status.success(), "{}\n{emitted_script}", String::from_utf8_lossy(&emitted.stderr));
    assert_eq!(String::from_utf8_lossy(&emitted.stderr), "");
    assert_eq!(String::from_utf8_lossy(&emitted.stdout), EXPECTED);
}

/// The SQLite CRuby tree carries the port too: boot fires Active
/// Record's hook, so the files' registrations run at once where the
/// files load.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_hook_in_a_file_level_branch_registers_on_the_sqlite_cruby_tree() {
    let script = format!(
        "%w[digesty stamped].each {{ |f| require RoundhouseAutoload.path(\"models/roundhouse_files/lib/ext/#{{f}}\") }}\n{SCRIPT}"
    );
    let run = emit_and_run::real_blog().write("lib/ext/digesty.rb", DIGESTY).write("lib/ext/stamped.rb", STAMPED).run_ruby(&script);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_tree_without_the_lazy_load_hooks_port_refuses_a_conditional_registration() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        (std::path::PathBuf::from("lib/ext/digesty.rb"), DIGESTY.as_bytes().to_vec()),
        (std::path::PathBuf::from("lib/ext/stamped.rb"), STAMPED.as_bytes().to_vec()),
    ]
    .into_iter()
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, std::path::Path::new("not-a-fixture"), BuildTarget::Spinel)
    });
    let error = result.err().expect("spinel emitted the registration");
    assert!(error.contains("`ActiveSupport.on_load` registration needs the CRuby tree"), "{error}");
    let refused = diagnostics
        .iter()
        .filter(|d| d.message.contains("load_hook_registration") && d.message.contains("`ActiveSupport.on_load(:active_record)`"))
        .count();
    // digesty's one, and both of stamped's branches.
    assert_eq!(refused, 3, "{diagnostics:#?}");
}
