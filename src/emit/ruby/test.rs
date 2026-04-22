//! `test/{controllers,models}/*.rb` emission. One file per
//! `TestModule`, with `test "name" do …` declarations rendered in
//! source order.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use super::expr::emit_expr;
use super::shared::emit_indented_body;
use crate::dialect::TestModule;
use crate::naming::snake_case;

pub(super) fn emit_test_module(tm: &TestModule) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "require \"test_helper\"").unwrap();
    writeln!(s).unwrap();
    let superclass = tm
        .parent
        .as_ref()
        .map(|c| format!(" < {}", c.0))
        .unwrap_or_default();
    writeln!(s, "class {}{}", tm.name.0, superclass).unwrap();
    for (i, test) in tm.tests.iter().enumerate() {
        if i > 0 {
            writeln!(s).unwrap();
        }
        writeln!(s, "  test {:?} do", test.name).unwrap();
        emit_indented_body(&mut s, &emit_expr(&test.body), 2);
        writeln!(s, "  end").unwrap();
    }
    writeln!(s, "end").unwrap();
    // File path: `test/<subdir>/<snake_class_name>.rb`. Subdir is
    // `controllers/` when the class name follows the Rails
    // convention `FooControllerTest`, otherwise `models/`. Matches
    // both directories the ingester walks, so Ruby round-trip lands
    // in the same spot the source did.
    let filename = snake_case(tm.name.0.as_str());
    let subdir = if tm.name.0.as_str().ends_with("ControllerTest") {
        "controllers"
    } else {
        "models"
    };
    EmittedFile {
        path: PathBuf::from(format!("test/{subdir}/{filename}.rb")),
        content: s,
    }
}
