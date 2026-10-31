//! PostgreSQL: query logs, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_query_logs
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = "class Widget < ApplicationRecord\nend\n";

const PROBE: &str = r##"require "erb"

class QueryLogsProbe
  def self.lines
    out = []
    logs = ActiveRecord::QueryLogs
    conn = ActiveRecord::Base.connection
    out << "default: #{logs.call("SELECT 1", conn).inspect}"

    logs.taggings = logs.taggings.merge(
      application: "Probe",
      static: "v 1",
      dyn: -> { "d1" },
      ctx: ->(context) { context[:connection] ? "conn" : "none" },
      sql_length: ->(context) { context[:sql].length },
      from_execution: ->(context) { context[:request_id] },
      escaped: "a*/b/*c",
      absent: -> { nil },
    )
    logs.tags = [:application, :dyn, :ctx, :static, :sql_length, { extra: -> { "x y" }, from_execution: nil }, :escaped, :absent]
    out << "legacy: #{logs.call("SELECT 1", conn).inspect}"

    ActiveSupport::ExecutionContext[:request_id] = "r-9"
    out << "context: #{logs.call("SELECT 2", conn).inspect}"
    ActiveSupport::ExecutionContext.clear
    out << "cleared: #{logs.call("SELECT 2", conn).inspect}"

    logs.prepend_comment = true
    out << "prepended: #{logs.call("SELECT 3", conn).inspect}"
    logs.prepend_comment = false

    logs.tags_formatter = :sqlcommenter
    out << "sqlcommenter: #{logs.call("SELECT 4", conn).inspect}"
    begin
      logs.tags_formatter = :nope
    rescue ArgumentError => e
      out << "formatter: #{e.message}"
    end
    out << "formatter kept: #{logs.tags_formatter.inspect}"

    logs.cache_query_log_tags = true
    first = logs.call("SELECT 5", conn)
    logs.taggings = logs.taggings.merge(dyn: -> { "d2" })
    out << "cached: #{first == logs.call("SELECT 5", conn)} #{logs.call("SELECT 5", conn).inspect}"
    ActiveSupport::ExecutionContext[:other] = 1
    out << "cache cleared by context change: #{logs.call("SELECT 5", conn).inspect}"
    logs.cache_query_log_tags = false

    logs.tags_formatter = :legacy
    ActiveRecord.query_transformers << logs
    seen = []
    listener = ActiveSupport::Notifications.subscribe("sql.active_record") { |*args| seen << args.last[:sql] }
    Widget.create!(name: "a")
    Widget.where(name: "a").to_a
    conn.select_value("SELECT 6")
    ActiveSupport::Notifications.unsubscribe(listener)
    ActiveRecord.query_transformers.delete(logs)
    # The statement text differs by how each side lowers the model call;
    # what the transformer appended is the comment, the same on both.
    seen.select { |sql| sql.include?("\"widgets\"") || sql.include?("SELECT 6") }.each do |sql|
      out << "sent: #{sql.split.first} #{sql[%r{/\*.*\*/\z}].sub(/\bsql_length:\d+/, "sql_length:N").inspect}"
    end
    out << "row: #{Widget.first.name}"
    out << "transformers: #{ActiveRecord.query_transformers.size}"
    out
  end
end
"##;

const EXPECTED_PREFIX: [&str; 2] = [
    "default: \"SELECT 1\"",
    "legacy: \"SELECT 1 /*application:Probe,ctx:conn,dyn:d1,escaped:a* /b/ *c,extra:x y,sql_length:8,static:v 1*/\"",
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/widget", "app/models/query_logs_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/query_logs_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn query_logs_and_transformers_match_native_rails_on_postgresql() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("query-logs", &FILES, &emitted, "rh_spec_query_logs", "QueryLogsProbe", &LOADS) else { return };
    // Not a vacuous agreement: Rails' own answers for the first lines.
    for (i, expected) in EXPECTED_PREFIX.iter().enumerate() {
        assert_eq!(native[i], *expected, "native line {i}");
    }
    assert!(native.iter().any(|l| l.starts_with("sent: ") && l.contains("/*application:Probe")), "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
