//! PostgreSQL: controller load hook include, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const AWARE: &str = r#"module AwareController
  extend ActiveSupport::Concern

  def aware?
    true
  end

  ActiveSupport.run_load_hooks(:aware_controller, self)
end
"#;

const ACTOR: &str = r#"module WithActor
  extend ActiveSupport::Concern

  def set_actor
    @actor = "api"
  end

  def actor
    @actor
  end
end
"#;

const LOGIN: &str = r#"module Login
  extend ActiveSupport::Concern
  include WithActor

  def logged_in?
    true
  end
end
"#;

const INITIALIZER: &str = r#"ActiveSupport.on_load(:aware_controller) do
  AwareController.include(Login)
end
"#;

const BASE: &str = r#"class BaseController < ApplicationController
  include AwareController
end
"#;

const RISKS: &str = r##"class RisksController < BaseController
  def show
    set_actor
    render plain: "#{actor} #{logged_in?} #{aware?}"
  end
end
"##;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_hooked_concerns_later_include_reaches_the_controllers() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_load_hook_include\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/risks\", to: \"risks#show\"\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/concerns/aware_controller.rb", AWARE)
        .write("app/controllers/concerns/with_actor.rb", ACTOR)
        .write("app/controllers/concerns/login.rb", LOGIN)
        .write("config/initializers/aware_controller.rb", INITIALIZER)
        .write("app/controllers/base_controller.rb", BASE)
        .write("app/controllers/risks_controller.rb", RISKS)
        .emit(BuildTarget::Ruby);
    let base = std::fs::read_to_string(dir.join("app/controllers/base_controller.rb")).expect("base_controller.rb");
    assert!(base.contains("include AwareController"), "the controller keeps its `include` of the hooked concern, which the later `on_load` include extends:\n{base}");
}
