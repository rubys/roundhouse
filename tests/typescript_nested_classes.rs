//! App classes nested under a class or a module, on the TypeScript emit.
//! Each was written to a file of its own named after the Ruby path
//! (`importer::too_big.ts`) holding a bare `class TooBig`, while callers
//! read `Importer.TooBig`, which nothing defined; `Importer::TooBig.new`
//! stayed a `.new` call and a module root (`Billing`) was never imported.
//! It all type-checked loosely and failed only when run. The run itself
//! is `typescript_toolchain::real_blog_nested_classes_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/nested_classes.rs"]
mod nested_classes;

use roundhouse::project::BuildTarget;

#[test]
fn nested_classes_live_in_their_roots_namespace() {
    let (emitted, errors) = nested_classes::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| {
        std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
    };
    let models: Vec<String> = std::fs::read_dir(emitted.join("app/models"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!models.iter().any(|f| f.contains("::")), "{models:?}");

    let importer = read("app/models/importer.ts");
    let ns = importer.find("export namespace Importer {").expect(&importer);
    assert!(importer.find("export class Importer {").unwrap() < ns, "{importer}");
    assert!(importer[ns..].contains("export class TooBig extends Error"), "{importer}");
    assert!(!importer.contains("const e = e"), "{importer}");

    let billing = read("app/models/billing.ts");
    for decl in ["export namespace Billing {", "export class Invoice {", "export namespace Invoice {", "export class Line {"] {
        assert!(billing.contains(decl), "{decl}:\n{billing}");
    }
    assert!(billing.contains("new Billing.Invoice.Line(x)"), "{billing}");

    let report = read("app/models/report.ts");
    assert!(report.contains("import { Billing } from \"./billing.js\";"), "{report}");
    assert!(report.contains("import { Importer } from \"./importer.js\";"), "{report}");
    assert!(report.contains("new Importer.TooBig(\"direct\")"), "{report}");
    assert!(report.contains("new Billing.Invoice.Line(3)"), "{report}");
}
