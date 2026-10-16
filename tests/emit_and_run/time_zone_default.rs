//! `config.time_zone` is the zone Active Record values are presented in
//! (Rails' `Time.zone`), and the process zone stays the host's: Rails
//! never sets `ENV["TZ"]`, so `Time.now` is the host's local time.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    \
             config.load_defaults 8.1\n    config.time_zone = \"Tokyo\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :label\n    t.datetime :created_at\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
}

const ASSERTIONS: &str = r#"
Db.exec("INSERT INTO widgets (id, label, created_at) VALUES (1, 'a', '2021-08-01 12:00:00')")
shown = Widget.find(1).created_at.strftime("%Y-%m-%d %H:%M %z")
raise "created_at not in config.time_zone: #{shown}" unless shown == "2021-08-01 21:00 +0900"
raise "boot pinned the process zone: ENV['TZ'] = #{ENV['TZ'].inspect}" if ENV["TZ"] == "Asia/Tokyo"
puts "config.time_zone presents records; the process zone is the host's"
"#;

#[test]
fn config_time_zone_presents_records_and_leaves_the_process_zone_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn config_time_zone_presents_records_and_leaves_the_process_zone_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}
