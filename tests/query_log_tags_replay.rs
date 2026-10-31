//! PostgreSQL: query log tags replay, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const REPLAY: &str = "ActiveRecord::QueryLogs.apply_railtie_config(";

fn models(application: Option<&str>) -> String {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_query_log_tags_replay\n");
    if let Some(body) = application {
        app = app.write(
            "config/application.rb",
            &format!("module Blog\n  class Application < Rails::Application\n{body}  end\nend\n"),
        );
    }
    let (dir, _) = app.emit(BuildTarget::Ruby);
    std::fs::read_to_string(dir.join("app/models.rb")).expect("app/models.rb")
}

#[test]
fn an_app_that_never_enables_query_log_tags_has_no_replay() {
    assert!(!models(None).contains(REPLAY));
    assert!(!models(Some("")).contains(REPLAY));
    let other = "    config.active_record.query_log_tags_format = :sqlcommenter\n";
    assert!(!models(Some(other)).contains(REPLAY));
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_app_that_enables_query_log_tags_replays_its_config() {
    let body = "    config.active_record.query_log_tags_enabled = true\n    config.active_record.query_log_tags_format = :sqlcommenter\n";
    let text = models(Some(body));
    let at = text.find(REPLAY).unwrap_or_else(|| panic!("no replay in:\n{text}"));
    let call = &text[at..text[at..].find('\n').map_or(text.len(), |n| at + n)];
    assert!(call.contains("enabled: Rails.application."), "{call}");
    assert!(call.contains("format: Rails.application."), "{call}");
    assert!(call.contains("tags: nil"), "{call}");
}
