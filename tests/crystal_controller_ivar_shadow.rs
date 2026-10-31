//! #1625: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) redeclared that state in the Crystal subclass, and
//! `crystal build` refused it (`instance variable '@headers' of
//! ActionController::Base ... is already declared as
//! ActionController::HeaderStore`). The app's ivar takes another name in
//! the emitted controller, as on TypeScript (#1614). The compile and a run
//! are `crystal_toolchain::controller_ivars_named_like_base_state_*`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_crystal_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("src/controllers/probes_controller.cr");
    let application = read("src/controllers/application_controller.cr");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["status", "location", "headers"] {
            assert!(!text.contains(&format!("@{name} ")), "{file} still declares or assigns @{name}:\n{text}");
        }
    }
    assert!(probes.contains("@app_status = \"app status\""), "{probes}");
    assert!(probes.contains("@app_headers = \"app headers\""), "{probes}");
    assert!(application.contains("@app_location = \"app location\""), "{application}");
}

/// Crystal names `performed?` as Ruby does, so an app `@is_performed`
/// names no runtime member there and keeps its name.
#[test]
fn an_ivar_naming_no_crystal_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("src/controllers/probes_controller.cr")).unwrap();
    assert!(probes.contains("@is_performed = \"app flag\""), "{probes}");
}
