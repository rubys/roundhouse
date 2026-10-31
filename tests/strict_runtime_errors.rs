//! The ActiveRecord/ActiveModel error classes (`runtime/ruby/active_record/errors.rb`)
//! transpile into every strict target's runtime. `ActiveModel::UnknownAttributeError`
//! once built its message from `record.class.name`, which the strict base classes
//! don't have, and subclasses `NoMethodError`, which Crystal resolves at parse time
//! (#1542). These check the emitted text; the toolchain tests compile it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::{Path, PathBuf};

fn find(root: &Path, file_name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == file_name) {
                found.push(path);
            }
        }
    }
    found
}

fn emitted_errors(target: BuildTarget, file_name: &str) -> String {
    let (emitted, _) = emit_and_run::empty_app().emit(target);
    let files = find(&emitted, file_name);
    assert_eq!(files.len(), 1, "{target:?}: expected one {file_name}, found {files:?}");
    std::fs::read_to_string(&files[0]).unwrap()
}

#[test]
fn strict_error_runtimes_do_not_reflect_on_the_record_class() {
    // Swift and Kotlin escape the reserved word (`` record.`class` ``).
    // Crystal has `.class`, so only its load order matters (below).
    let mut failures = Vec::new();
    for (target, file_name, reflection) in [
        (BuildTarget::Swift, "Errors.swift", ".`class`"),
        (BuildTarget::Kotlin, "Errors.kt", ".`class`"),
        (BuildTarget::CSharp, "Errors.cs", ".Class"),
    ] {
        let source = emitted_errors(target, file_name);
        assert!(source.contains("UnknownAttributeError"), "{target:?}: {file_name} has no UnknownAttributeError");
        if let Some(line) = source.lines().find(|line| line.contains(reflection)) {
            failures.push(format!("{target:?}: {file_name} reads `{reflection}`: {}", line.trim()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn crystal_requires_the_core_errors_before_the_errors_that_subclass_them() {
    let (emitted, _) = emit_and_run::empty_app().emit(BuildTarget::Crystal);
    let app_cr = std::fs::read_to_string(emitted.join("src/app.cr")).unwrap();
    let position = |line: &str| app_cr.lines().position(|l| l == line);
    let core = position("require \"./core_errors\"").expect("app.cr requires core_errors");
    let errors = position("require \"./errors\"").expect("app.cr requires errors");
    assert!(core < errors, "core_errors must load before errors:\n{app_cr}");
}
