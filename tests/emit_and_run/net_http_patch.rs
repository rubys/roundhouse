//! `Net::HTTP::Patch` and `Net::ProtocolError` run on the emitted Ruby
//! tree (invariant 6). CRuby-only `HTTPBadResponse` /
//! `HTTPHeaderSyntaxError` stay refused at check time — do not pin them
//! here until Spinel defines them.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"homes#show\"\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/homes_controller.rb",
            "class HomesController < ApplicationController\n  def show\n    klass = Net::HTTP::Patch\n    begin\n      raise Net::ProtocolError, \"bad\"\n    rescue Net::ProtocolError => e\n      render plain: \"#{klass.name}:#{e.class.name}\"\n    end\n  end\nend\n",
        )
        .write("app/views/homes/show.html.erb", "unused\n")
}

const SCRIPT: &str = r#"
# HomesController requires net/http (and the runtime reopen) the same
# way Jumpstart's ApplicationClient does at class load — load it so the
# pin exercises the emitted graph, not a bare stdlib require.
require_relative "app/controllers/homes_controller"
raise "Patch missing" unless defined?(Net::HTTP::Patch)
klass = Net::HTTP::Patch
begin
  raise Net::ProtocolError, "bad"
rescue Net::ProtocolError => e
  raise "unexpected: #{e.class.name}:#{e.message}" unless e.message == "bad"
  raise "unexpected klass #{klass.name}" unless klass.name == "Net::HTTP::Patch"
end
puts "net-http-patch-ok"
"#;

#[test]
fn net_http_patch_and_protocol_error_run_on_emitted_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(
        run.stdout.contains("net-http-patch-ok"),
        "stdout={}",
        run.stdout
    );
}
