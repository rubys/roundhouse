//! #1228: an app class named `Base` takes the bare name in the app's
//! TypeScript files, and the runtime's `ActiveRecord::Base` and
//! `ActionController::Base` are imported there under aliases. Before,
//! every file naming `Base` imported the runtime's, so `class Widget <
//! Base` extended the record base and `Widget.label` was undefined. The
//! run and `tsc` are `typescript_toolchain::real_blog_app_class_base_runs_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/app_class_base.rs"]
mod app_class_base;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_class_named_base_is_imported_from_the_app() {
    let (emitted, errors) = app_class_base::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let widget = read("app/models/widget.ts");
    assert!(widget.contains("import { Base } from \"./base.js\";"), "{widget}");
    assert!(widget.contains("export class Widget extends Base {"), "{widget}");
    let record = read("app/models/application_record.ts");
    assert!(record.contains("import { Base as ActiveRecordBase } from \"../../src/active_record_base.js\";"), "{record}");
    assert!(record.contains("extends ActiveRecordBase {"), "{record}");
    let controller = read("app/controllers/application_controller.ts");
    assert!(controller.contains("import { Base as ActionControllerBase } from \"../../src/action_controller_base.js\";"), "{controller}");
    assert!(controller.contains("extends ActionControllerBase {"), "{controller}");
}

/// An app without a class of that name keeps the plain runtime import.
#[test]
fn without_an_app_base_the_runtime_base_keeps_its_name() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let record = std::fs::read_to_string(emitted.join("app/models/application_record.ts")).unwrap();
    assert!(record.contains("import { Base } from \"../../src/active_record_base.js\";"), "{record}");
    assert!(record.contains("extends Base {"), "{record}");
}

/// The alias steps aside for an app class of its name: an app
/// `ActiveRecordBase` extending the runtime base extended itself
/// (TS2506), and a file naming it imported the runtime base instead.
#[test]
fn the_runtime_base_alias_avoids_an_app_class_of_that_name() {
    let (emitted, errors) = app_class_base::alias_taken_overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let arb = read("app/models/active_record_base.ts");
    assert!(arb.contains("import { Base as ActiveRecordBase2 } from \"../../src/active_record_base.js\";"), "{arb}");
    assert!(arb.contains("export class ActiveRecordBase extends ActiveRecordBase2 {"), "{arb}");
    let gadget = read("app/models/gadget.ts");
    assert!(gadget.contains("import { ActiveRecordBase } from \"./active_record_base.js\";"), "{gadget}");
    assert!(!gadget.contains("src/active_record_base.js"), "{gadget}");
}

