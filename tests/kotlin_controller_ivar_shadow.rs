//! #1625: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) is a property of the Kotlin controller, which shares one
//! namespace with the runtime base's `status`, `location` and `headers`:
//! the app assigned its String to them, or overrode them with its own
//! type. The app's ivar takes another name in the emitted controller, as
//! on TypeScript (#1614) and Crystal (#1638). The compile and a run are
//! `kotlin_toolchain::controller_ivars_named_like_base_state_*`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_kotlin_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("src/main/kotlin/app/controllers/ProbesController.kt");
    let application = read("src/main/kotlin/app/controllers/ApplicationController.kt");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["status", "location", "headers"] {
            assert!(!text.contains(&format!("this.{name} =")), "{file} still assigns this.{name}:\n{text}");
            assert!(!text.contains(&format!("var {name}:")), "{file} still declares {name}:\n{text}");
        }
    }
    assert!(probes.contains("this.appStatus = \"app status\""), "{probes}");
    assert!(probes.contains("this.appHeaders = \"app headers\""), "{probes}");
    assert!(application.contains("this.appLocation = \"app location\""), "{application}");
    assert!(probes.contains("Probes.show(appHeaders, appLocation, appStatus,"), "{probes}");
}

/// The runtime's `performed?` is `performedPred()` in Kotlin, so an app
/// `@is_performed` (`isPerformed`) names no runtime member and keeps its name.
#[test]
fn an_ivar_naming_no_kotlin_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("src/main/kotlin/app/controllers/ProbesController.kt")).unwrap();
    assert!(probes.contains("this.isPerformed = \"app flag\""), "{probes}");
}
