use super::emit_and_run;

#[test]
fn stored_and_explicitly_zoned_times_keep_utc_identity() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("config/application.rb", "module UtcProbe\n  class Application < Rails::Application\n    config.load_defaults 8.1\n    config.time_zone = \"UTC\"\n  end\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.datetime :created_at\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/services/utc_zone_probe.rb", r#"class UtcZoneProbe
  def self.from_record
    Widget.find(1).created_at.iso8601(6)
  end

  def self.from_utc_zone
    Time.utc(2026, 1, 2, 3, 4, 5).in_time_zone("UTC").iso8601
  end

  def self.from_gmt_zone
    Time.utc(2026, 1, 2, 3, 4, 5).in_time_zone("GMT").iso8601
  end
end
"#)
        .run_ruby(r#"
Db.exec("INSERT INTO widgets (id, created_at) VALUES (1, '2026-01-02 03:04:05.123456')")
raise UtcZoneProbe.from_record unless UtcZoneProbe.from_record == "2026-01-02T03:04:05.123456Z"
raise UtcZoneProbe.from_utc_zone unless UtcZoneProbe.from_utc_zone == "2026-01-02T03:04:05Z"
raise UtcZoneProbe.from_gmt_zone unless UtcZoneProbe.from_gmt_zone == "2026-01-02T03:04:05+00:00"
puts "Stored and explicitly zoned times keep UTC identity"
"#)
        .assert_passes();
}
