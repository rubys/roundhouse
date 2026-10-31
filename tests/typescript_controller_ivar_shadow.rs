//! #1614: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) is a property of the same TypeScript class as the
//! runtime's field, and `tsc` refused the override (TS2416). Ruby keeps
//! an ivar apart from a method of its name; TypeScript doesn't, so the
//! app's ivar takes another name in the emitted controller, wherever it
//! is set or read, and the view gets it as before, by argument. The
//! compile and the rendered page are
//! `typescript_toolchain::controller_ivars_named_like_base_state_compile_and_render`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("app/controllers/probes_controller.ts");
    let application = read("app/controllers/application_controller.ts");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["status", "location", "headers"] {
            assert!(!text.contains(&format!("declare {name}:")), "{file} still declares {name}:\n{text}");
            assert!(!text.contains(&format!("this.{name} =")), "{file} still assigns this.{name}:\n{text}");
        }
    }
    assert!(probes.contains("declare app_status: string;"), "{probes}");
    assert!(probes.contains("declare app_headers: string;"), "{probes}");
    assert!(application.contains("declare app_location: string;"), "{application}");
    assert!(probes.contains("this.app_status = \"app status\";"), "{probes}");
    assert!(probes.contains("this.app_location, this.app_status"), "{probes}");
}

/// A runtime member is matched by the name the TypeScript emit gives it:
/// `performed?` is `is_performed()` there, so an app `@is_performed`
/// collides with it as much as `@status` does with `status`.
#[test]
fn an_app_ivar_named_like_a_sanitized_base_method_is_renamed() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/probes_controller.ts")).unwrap();
    assert!(!probes.contains("declare is_performed:"), "{probes}");
    assert!(probes.contains("declare app_is_performed: string;"), "{probes}");
    assert!(probes.contains("this.app_is_performed = \"app flag\";"), "{probes}");
    assert!(probes.contains("if (this.is_performed()) { return null; }"), "{probes}");
}

/// An ivar that names no runtime member keeps its name.
#[test]
fn an_ivar_naming_no_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay()
        .edit("app/controllers/probes_controller.rb", "@status = \"app status\"", "@note = \"app status\"")
        .write("app/views/probes/show.html.erb", &ts_controller_ivars::SHOW.replace("@status", "@note"))
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/probes_controller.ts")).unwrap();
    assert!(probes.contains("declare note: string;"), "{probes}");
}
