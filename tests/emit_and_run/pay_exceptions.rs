//! `rescue Pay::Error` runs on the emitted Ruby tree (invariant 6).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "Gemfile.lock",
            "GEM\n  remote: https://rubygems.org/\n  specs:\n    pay (12.1.1)\n\nDEPENDENCIES\n  pay\n",
        )
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
            "class HomesController < ApplicationController\n  def show\n    begin\n      raise Pay::Error, \"card declined\"\n    rescue Pay::Error => e\n      render plain: \"caught:#{e.message}\"\n    end\n  end\nend\n",
        )
        .write("app/views/homes/show.html.erb", "unused\n")
}

const SCRIPT: &str = r#"
begin
  raise Pay::Error, "card declined"
rescue Pay::Error => e
  raise "unexpected: #{e.message}" unless e.message == "card declined"
end
begin
  raise Pay::ActionRequired, "confirm"
rescue Pay::ActionRequired => e
  raise "unexpected: #{e.message}" unless e.message == "confirm"
end
raise "Charge constant missing" unless defined?(Pay::Charge)
puts "pay-exceptions-ok"
"#;

#[test]
fn rescue_pay_error_runs_on_emitted_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(
        run.stdout.contains("pay-exceptions-ok"),
        "stdout={}",
        run.stdout
    );
}
