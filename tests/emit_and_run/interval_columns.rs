use super::emit_and_run;

/// A PostgreSQL interval column round-trips as `ActiveSupport::Duration`.
/// The shape is Rails' `t.interval`, not one application's table.
#[test]
fn an_interval_column_round_trips_a_duration() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "job_runs", force: :cascade do |t|
    t.interval "duration"
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/job_run.rb", "class JobRun < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r#"
hour = ActiveSupport::Duration.hours(1)
row = JobRun.create!(duration: hour)
raise "create #{row.duration.inspect}" unless row.duration == hour
found = JobRun.find(row.id)
raise "reload #{found.duration.inspect}" unless found.duration == hour
raise "index #{found[:duration].inspect}" unless found[:duration] == hour
raise "attributes #{found.attributes["duration"].inspect}" unless found.attributes["duration"] == hour
raise "where" unless JobRun.where(duration: hour).first.id == found.id
ninety = ActiveSupport::Duration.minutes(90)
found.duration = ninety
found.save!
raise "update #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration == ninety
found.duration = "PT2H"
found.save!
raise "iso #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration == ActiveSupport::Duration.hours(2)
found[:duration] = "01:30:00"
found.save!
raise "postgres text #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration == ninety
found.duration = nil
found.save!
raise "clear #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration.nil?
found.duration = "PT1.5H"
found.save!
raise "fractional hour #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration.nil?
found.duration = "-1 days -02:00:00"
found.save!
neg = ActiveSupport::Duration.seconds(-93600)
raise "negative postgres #{JobRun.find(found.id).duration.inspect}" unless JobRun.find(found.id).duration == neg
JobRun.create!(duration: ActiveSupport::Duration.days(2))
JobRun.create!(duration: ActiveSupport::Duration.days(1))
ordered = JobRun.order(:duration).map { |row| row.duration.nil? ? nil : row.duration.seconds }
raise "order #{ordered.inspect}" unless ordered.compact == [-93600, 86400, 172800]
puts "interval column round-trip ok"
"#,
        )
        .assert_passes();
}
