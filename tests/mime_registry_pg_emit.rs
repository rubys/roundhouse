//! PostgreSQL: mime registry pg emit, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_mime_registry\n";
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

fn emit() -> emit_and_run::Emitted {
    let app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", "\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n");
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    emitted
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_registry_and_encoder_ports_are_carried() {
    let emitted = emit();
    let mime = std::fs::read_to_string(emitted.join("runtime/mime.rb")).unwrap();
    assert!(mime.contains("module Registry"), "{mime}");
    assert!(emitted.join("runtime/action_dispatch_request_encoder.rb").exists());
    let hooks = std::fs::read_to_string(emitted.join("test/test_helper.rb")).unwrap();
    assert!(hooks.contains("action_dispatch_request_encoder"), "{hooks}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn registered_types_resolve_through_mime() {
    let emitted = emit();
    let script = r#"
require "./runtime/mime"
lines = []
lines << "before: #{Mime[:graphql].inspect}"
Mime::Type.register("application/graphql", :graphql)
lines << "registered: #{Mime[:graphql]} #{Mime::Type.lookup("application/graphql").symbol} #{Mime::Type.lookup_by_extension("graphql").to_s}"
Mime::Type.register_alias("text/x-probe", :probe)
lines << "alias: #{Mime[:probe]} #{Mime::Type.lookup("text/x-probe").symbol.inspect}"
lines << "frozen: #{Mime[:json]} #{Mime::Type.lookup("text/html").symbol}"
puts lines
"#;
    let out = std::process::Command::new("ruby").arg("-e").arg(script).current_dir(&*emitted).output().expect("ruby");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let got = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        got.lines().collect::<Vec<_>>(),
        [
            "before: nil",
            "registered: application/graphql graphql application/graphql",
            "alias: text/x-probe nil",
            "frozen: application/json html",
        ],
        "{got}"
    );
}
