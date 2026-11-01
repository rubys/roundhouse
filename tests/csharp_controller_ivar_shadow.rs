//! #1625: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) is a property of the C# controller, which shares one
//! namespace with the runtime base's `Status`, `Location` and its
//! `Headers()` method: the app assigned its String to them, or declared a
//! `Headers` property beside the base's method. The app's ivar takes
//! another name in the emitted controller, as on TypeScript (#1614) and
//! Crystal (#1638). The build and a run are
//! `csharp_toolchain::controller_ivars_named_like_base_state_*`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_csharp_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("app/controllers/ProbesController.cs");
    let application = read("app/controllers/ApplicationController.cs");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["Status", "Location", "Headers"] {
            assert!(!text.contains(&format!("this.{name} =")), "{file} still assigns this.{name}:\n{text}");
            assert!(!text.contains(&format!("string {name} {{")), "{file} still declares {name}:\n{text}");
        }
    }
    assert!(probes.contains("this.AppStatus = \"app status\""), "{probes}");
    assert!(probes.contains("this.AppHeaders = \"app headers\""), "{probes}");
    assert!(application.contains("this.AppLocation = \"app location\""), "{application}");
    assert!(probes.contains("Probes.Show(AppHeaders, AppLocation, AppStatus,"), "{probes}");
}

/// The runtime's `performed?` is `PerformedPred()` in C#, so an app
/// `@is_performed` (`IsPerformed`) names no runtime member and keeps its name.
#[test]
fn an_ivar_naming_no_csharp_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/ProbesController.cs")).unwrap();
    assert!(probes.contains("this.IsPerformed = \"app flag\""), "{probes}");
}

/// With no app/controllers/application_controller.rb, the C# emit writes
/// `public class ApplicationController : ActionControllerBase` itself; a
/// controller under it is still a controller, and its ivars are renamed.
#[test]
fn an_app_ivar_is_renamed_under_a_synthesized_application_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay()
        .remove("app/controllers/application_controller.rb")
        .write("app/views/probes/show.html.erb", &ts_controller_ivars::SHOW.replace("<p><%= @location %></p>\n", ""))
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/ProbesController.cs")).unwrap();
    assert!(!probes.contains("this.Status ="), "probes still assigns this.Status:\n{probes}");
    assert!(probes.contains("this.AppStatus = \"app status\""), "{probes}");
}

/// ApplicationController's `set_location`, a `before_action` that isn't
/// inlined, is a value method in C# (`object? SetLocation()`). Its last
/// statement is an ivar assignment, which in Ruby answers the value, so
/// the method returns it; with no `return` the C# doesn't build (CS0161).
#[test]
fn a_method_ending_in_an_ivar_assignment_returns_it() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let application = std::fs::read_to_string(emitted.join("app/controllers/ApplicationController.cs")).unwrap();
    assert!(
        application.contains("this.AppLocation = \"app location\";\n        return AppLocation;"),
        "{application}"
    );
}
