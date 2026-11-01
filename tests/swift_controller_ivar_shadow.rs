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

/// An ivar a `String` in one method and an Integer in another is a
/// `String` property; the method ending in the Integer assignment returns
/// `Int`, so it returns the value it assigned, not the property.
#[test]
fn a_method_returns_the_value_it_assigned_not_the_property() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def label\n    @name_tag = \"none\"\n  end\n\n  def renumber\n    @name_tag = 2\n  end\nend\n")
        .emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("Sources/App/app/models/Widget.swift")).unwrap();
    assert!(widget.contains("var nameTag: String"), "{widget}");
    assert!(widget.contains("func renumber() -> Int {\n        let __rhRet = 2\n        self.nameTag = __rhRet\n        return __rhRet\n    }"), "{widget}");
}
