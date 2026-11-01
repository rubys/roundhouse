//! #1625: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) is a property of the Swift controller, which shares one
//! namespace with the runtime base's `status: Int`, `location: String?` and
//! `headers: HeaderStore!`: the app assigned its String to them. The app's
//! ivar takes another name in the emitted controller, as on TypeScript
//! (#1614) and Crystal (#1638). The build and a run are
//! `swift_toolchain::controller_ivars_named_like_base_state_*`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_swift_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("Sources/App/app/controllers/ProbesController.swift");
    let application = read("Sources/App/app/controllers/ApplicationController.swift");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["status", "location", "headers"] {
            assert!(!text.contains(&format!("self.{name} =")), "{file} still assigns self.{name}:\n{text}");
            assert!(!text.contains(&format!("var {name}:")), "{file} still declares {name}:\n{text}");
        }
    }
    assert!(probes.contains("self.appStatus = \"app status\""), "{probes}");
    assert!(probes.contains("self.appHeaders = \"app headers\""), "{probes}");
    assert!(application.contains("self.appLocation = "), "{application}");
    assert!(probes.contains("Probes.show(appHeaders, appLocation, appStatus,"), "{probes}");
}

/// The runtime's `performed?` is `performedPred()` in Swift, so an app
/// `@is_performed` (`isPerformed`) names no runtime member and keeps its name.
#[test]
fn an_ivar_naming_no_swift_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("Sources/App/app/controllers/ProbesController.swift")).unwrap();
    assert!(probes.contains("self.isPerformed = \"app flag\""), "{probes}");
}

/// With no app/controllers/application_controller.rb, the Swift emit
/// writes `class ApplicationController: ActionControllerBase` itself; a
/// controller under it is still a controller, and its ivars are renamed.
#[test]
fn an_app_ivar_is_renamed_under_a_synthesized_application_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay()
        .remove("app/controllers/application_controller.rb")
        .write("app/views/probes/show.html.erb", &ts_controller_ivars::SHOW.replace("<p><%= @location %></p>\n", ""))
        .emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("Sources/App/app/controllers/ProbesController.swift")).unwrap();
    assert!(!probes.contains("self.status ="), "probes still assigns self.status:\n{probes}");
    assert!(probes.contains("self.appStatus = \"app status\""), "{probes}");
}

/// ApplicationController's `set_location`, a `before_action` that isn't
/// inlined, is a value method in Swift (`func setLocation() -> Any?`). Its
/// last statement is an ivar assignment, which in Ruby answers the
/// assigned value, so the method returns that value (held once, since the
/// ivar's property can be typed apart from the method's return); with no
/// `return` the Swift doesn't build.
#[test]
fn a_method_ending_in_an_ivar_assignment_returns_it() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let application =
        std::fs::read_to_string(emitted.join("Sources/App/app/controllers/ApplicationController.swift")).unwrap();
    assert!(
        application.contains("let __rhRet = \"app location\"\n        self.appLocation = __rhRet\n        return __rhRet"),
        "{application}"
    );
}

/// The value held for the return is the one the property store coerces:
/// a `params` read surfaces as `Any??` and goes into an `Int` property
/// through `as! Int`, as the store alone wrote it.
#[test]
fn the_held_value_keeps_the_property_coercion() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/show\", to: \"probes#show\"\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  before_action :pick\n\n  def show\n  end\n\n  private\n\n  def pick\n    @count = 0\n    tally\n  end\n\n  def tally\n    @count = params[:count]\n  end\nend\n")
        .write("app/views/probes/show.html.erb", "<p><%= @count %></p>\n")
        .emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("Sources/App/app/controllers/ProbesController.swift")).unwrap();
    assert!(probes.contains("var count: Int = 0"), "{probes}");
    assert!(
        probes.contains("let __rhRet = (params[\"count\"] as! Int)\n        self.count = __rhRet\n        return __rhRet"),
        "{probes}"
    );
}
