//! Type casters, thread-local state, notifications, the null cache,
//! configuration files, and the ActiveJob constants, on the ruby target.

use super::emit_and_run;

fn shell() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
}

#[test]
fn active_record_string_and_integer_types_cast() {
    shell()
        .write(
            "app/models/caster.rb",
            r#"class Caster
  def self.values
    [
      ActiveRecord::Type::String.new.cast(nil),
      ActiveRecord::Type::String.new.cast(true),
      ActiveRecord::Type::String.new.cast(false),
      ActiveRecord::Type::String.new.cast(12),
      ActiveRecord::Type::Integer.new.cast(""),
      ActiveRecord::Type::Integer.new.cast("  "),
      ActiveRecord::Type::Integer.new.cast(:no),
      ActiveRecord::Type::Integer.new.cast(true),
      ActiveRecord::Type::Integer.new.deserialize("12"),
    ]
  end
end
"#,
        )
        .run_ruby(
            r#"
got = Caster.values
raise got.inspect unless got == [nil, "t", "f", "12", nil, nil, nil, 1, 12]
"#,
        )
        .assert_passes();
}

#[test]
fn support_and_job_constants_run() {
    shell()
        .write(
            "app/jobs/step_job.rb",
            r#"class StepJob
  include ActiveJob::Continuable

  def self.enqueue_failed
    raise ActiveJob::EnqueueError, "full"
  end

  def run
    [step(:one) { 1 }, step(:one) { 2 }]
  end
end
"#,
        )
        .write(
            "app/models/surface.rb",
            r#"class Surface
  def self.state
    ActiveSupport::IsolatedExecutionState[:box] = "kept"
    ActiveSupport::IsolatedExecutionState[:box]
  end

  def self.heard
    heard = []
    ActiveSupport::Notifications.subscribe("demo") { |*args| heard << args[0] }
    ActiveSupport::Notifications.instrument("demo") { "ok" }
    heard
  end

  def self.cached
    store = ActiveSupport::Cache::NullStore.new
    store.write("k", "v")
    [store.read("k"), store.fetch("k") { "fresh" }]
  end

  def self.config(path)
    ActiveSupport::ConfigurationFile.parse(path)
  end
end
"#,
        )
        .run_ruby(
            r#"
raise Surface.state.inspect unless Surface.state == "kept"
raise Surface.heard.inspect unless Surface.heard == ["demo"]
raise Surface.cached.inspect unless Surface.cached == [nil, "fresh"]
path = "config/probe.yml"
File.write(path, "size: <%= 1 + 1 %>\n")
raise Surface.config(path).inspect unless Surface.config(path)["size"] == 2
File.write(path, "adapter: :redis\n")
raise Surface.config(path).inspect unless Surface.config(path)["adapter"] == :redis
sqls = []
ActiveSupport::Notifications.subscribed(->(event) { sqls << event.payload[:sql] }, "sql.active_record") do
  Db.record_query("SELECT 1")
end
raise sqls.inspect unless sqls == ["SELECT 1"]
begin
  StepJob.enqueue_failed
  raise "enqueue did not fail"
rescue ActiveJob::EnqueueError => error
  raise error.message unless error.message == "full"
end
raise StepJob.new.run.inspect unless StepJob.new.run == [1, nil]
"#,
        )
        .assert_passes();
}
