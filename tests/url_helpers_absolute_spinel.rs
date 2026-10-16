//! `Rails.application.routes.url_helpers.x_url(…)` with `only_path:`
//! and `port:`, compiled by Spinel. The CRuby side is
//! `emit_and_run::url_helpers_only_path` and
//! `emit_and_run::url_helpers_absolute`; the expected strings are Rails
//! 8.1.4's.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn url_helpers_only_path_and_port_under_spinel() {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def only_path_link
    Rails.application.routes.url_helpers.widget_url(self, only_path: true)
  end

  def collection_only_path_link
    Rails.application.routes.url_helpers.widgets_url(only_path: true)
  end

  def link_with_port
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", port: 8080)
  end

  def link_on(port)
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", port: port)
  end

  def port_or_nil(port)
    port == 0 ? nil : port
  end

  def link_on_maybe(port)
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", port: port_or_nil(port))
  end

  def link_with(protocol, port)
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", protocol: protocol, port: port)
  end

  def link_on_env_port
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", port: ENV.fetch("WIDGET_LINK_PORT", "8080"))
  end

  def https_link_with_standard_port
    Rails.application.routes.url_helpers.widget_url(self, host: "example.com", protocol: "https", port: 443)
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show]\nend\n",
        );
    let run = overlay.run_spinel(
        r#"
w = Widget.new(name: "a")
w.id = 1
got = w.only_path_link
raise "only_path: #{got}" unless got == "/widgets/1"
got = w.collection_only_path_link
raise "collection: #{got}" unless got == "/widgets"
got = w.link_with_port
raise "port: #{got}" unless got == "http://example.com:8080/widgets/1"
got = w.https_link_with_standard_port
raise "standard port: #{got}" unless got == "https://example.com/widgets/1"
got = w.link_on(8080)
raise "run-time port: #{got}" unless got == "http://example.com:8080/widgets/1"
got = w.link_on(443)
raise "run-time 443: #{got}" unless got == "http://example.com:443/widgets/1"
got = w.link_on(80)
raise "run-time 80: #{got}" unless got == "http://example.com/widgets/1"
got = w.link_on_maybe(0)
raise "run-time nil: #{got}" unless got == "http://example.com/widgets/1"
got = w.link_on_maybe(8080)
raise "run-time maybe: #{got}" unless got == "http://example.com:8080/widgets/1"
got = w.link_with("https", 443)
raise "run-time protocol: #{got}" unless got == "https://example.com/widgets/1"
got = w.link_with("http", 443)
raise "run-time protocol 443: #{got}" unless got == "http://example.com:443/widgets/1"
got = w.link_on_env_port
raise "env port: #{got}" unless got == "http://example.com:8080/widgets/1"
puts "ok"
"#,
    );
    run.assert_passes();
}
