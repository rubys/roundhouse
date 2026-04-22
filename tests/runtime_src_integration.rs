//! Integration-level invariant: the functions the emitter produces
//! from runtime/ruby/*.rb + *.rbs MUST appear verbatim in the
//! corresponding per-target runtime files.
//!
//! This is what makes the Ruby source the source of truth. Hand-edits
//! to the target runtime files without updating the Ruby/RBS source
//! will fail this test — the way `pluralize` changes is by editing
//! runtime/ruby/inflector.rb and re-running CI, not by touching
//! runtime/python/view_helpers.py directly.
//!
//! For now only Python is covered. TypeScript / Crystal / Go / Rust /
//! Elixir join as their emit_method gains the single standalone-fn
//! entry point. Each addition is ~5 lines in this file.

use std::fs;
use std::path::Path;

use roundhouse::dialect::MethodDef;
use roundhouse::runtime_src::parse_methods_with_rbs;

fn load_typed(name: &str) -> Vec<MethodDef> {
    let ruby = fs::read_to_string(Path::new("runtime/ruby").join(format!("{name}.rb")))
        .expect("runtime/ruby/<name>.rb exists");
    let rbs = fs::read_to_string(Path::new("runtime/ruby").join(format!("{name}.rbs")))
        .expect("runtime/ruby/<name>.rbs exists");
    parse_methods_with_rbs(&ruby, &rbs).expect("Ruby+RBS parses and types cleanly")
}

#[test]
fn inflector_pluralize_lives_in_runtime_python() {
    let methods = load_typed("inflector");
    let pluralize = methods
        .iter()
        .find(|m| m.name.as_str() == "pluralize")
        .expect("inflector.rb defines pluralize");

    let emitted = roundhouse::emit::python::emit_method(pluralize);
    let file = fs::read_to_string("runtime/python/view_helpers.py")
        .expect("runtime/python/view_helpers.py exists");

    assert!(
        file.contains(&emitted),
        "runtime/python/view_helpers.py does not contain the emitted pluralize.\n\
         Expected (from runtime/ruby/inflector.rb + .rbs):\n\
         ----\n{emitted}----\n\
         If the emitter is now the source of truth, the runtime file must be \
         updated to match; if instead the runtime file's pluralize was edited \
         deliberately, the Ruby/RBS source needs the same edit."
    );
}
