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

fn pluralize_method() -> MethodDef {
    let methods = load_typed("inflector");
    methods
        .into_iter()
        .find(|m| m.name.as_str() == "pluralize")
        .expect("inflector.rb defines pluralize")
}

fn assert_emitted_lives_in(emitted: &str, file_path: &str) {
    let file = fs::read_to_string(file_path).unwrap_or_else(|_| panic!("{file_path} exists"));
    // Target runtime files typically nest the function inside a
    // module, so compare line-by-line modulo leading whitespace: the
    // emitter output must appear as a consecutive run of file lines
    // with only their indentation removed.
    let emitted_lines: Vec<&str> = emitted.lines().map(str::trim_start).collect();
    let file_lines: Vec<&str> = file.lines().map(str::trim_start).collect();
    let found = file_lines
        .windows(emitted_lines.len())
        .any(|w| w == emitted_lines.as_slice());
    assert!(
        found,
        "{file_path} does not contain the emitted function.\n\
         Expected (from runtime/ruby/inflector.rb + .rbs, compared modulo indent):\n\
         ----\n{emitted}----\n\
         If the emitter is now the source of truth, the runtime file must be \
         updated to match; if instead the runtime file was edited deliberately, \
         the Ruby/RBS source needs the same edit."
    );
}

#[test]
fn inflector_pluralize_lives_in_runtime_python() {
    let emitted = roundhouse::emit::python::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/python/view_helpers.py");
}

#[test]
fn inflector_pluralize_lives_in_runtime_crystal() {
    let emitted = roundhouse::emit::crystal::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/crystal/view_helpers.cr");
}

#[test]
fn inflector_pluralize_lives_in_runtime_rust() {
    let emitted = roundhouse::emit::rust::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/rust/view_helpers.rs");
}
