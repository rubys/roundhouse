//! Go target — `go2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `rust2` migration pattern. Strangler-fig orchestrator
//! that runs alongside the legacy `src/emit/go.rs` while the migration
//! to Group 1 (lowered IR + transpiled `runtime/ruby/`) lands.
//!
//! Selected at runtime via `ROUNDHOUSE_GO_V2=1`. Without the env var,
//! `super::go::emit` runs unchanged and emits the same output it
//! always has. With the env var, this module's overlay runs after
//! the legacy emit and writes transpiled framework runtime files
//! into the output project under `app/v2/` (separate Go package, so
//! emission can't conflict with the hand-written runtime types).
//!
//! Phase 1 scope: scaffold + minimal transpile via stub
//! `library::emit_library_class` (see `library.rs` for the contract).
//! Transpiled files are emitted but their method bodies are
//! `panic("go2 stub")` — `go build ./app/v2/...` produces a real
//! error inventory we can drive future sessions against.
//!
//! Out of scope for Phase 1: replacing the hand-written
//! `runtime/go/*.go` files (cable, http, server, view_helpers,
//! runtime, test_support, db) with transpiled equivalents. Those
//! land once the per-method body emit is real enough that calls
//! through them survive `go build`.

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod ty;

/// Append go2 transpiled runtime files to `files` when
/// `ROUNDHOUSE_GO_V2=1`. No-op otherwise — the default emit pipeline
/// (legacy go) ships unchanged.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
    if std::env::var("ROUNDHOUSE_GO_V2").as_deref() != Ok("1") {
        return;
    }
    files.extend(emit_overlay_files(app));
}

/// Produce the v2 overlay files unconditionally — for tests and
/// other callers that want the overlay output without setting an env
/// var. Returns the same files `overlay_v2` would append, in
/// emission order.
pub fn emit_overlay_files(_app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();
    let units = match crate::runtime_loader::go_units(|_, c| c) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather
            // than panicking — `go build` picks it up and the cargo
            // test that exercises overlay sees a non-empty result.
            out.push(EmittedFile {
                path: PathBuf::from("app/v2/transpile_error.txt"),
                content: format!("go2 transpile failed: {e}\n"),
            });
            return out;
        }
    };

    for unit in units {
        // The runtime_loader produces paths shaped like `app/X.go`
        // from GO_RUNTIME; relocate everything under `app/v2/` and
        // re-anchor the package to `v2` so this overlay can never
        // collide with legacy runtime types of the same name
        // (Inflector, JsonBuilder, Router, ...).
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.go".to_string());
        let out_path = PathBuf::from(format!("app/v2/{file_name}"));
        let content = rewrite_package_to_v2(&unit.content);
        out.push(EmittedFile {
            path: out_path,
            content,
        });
    }
    out
}

/// Replace the leading `package app` declaration with `package v2`
/// and inject `import` declarations for stdlib packages the body
/// references (`fmt`, `strings`, `regexp`). Go is strict about
/// unused imports so detection is by substring presence, not
/// always-on.
fn rewrite_package_to_v2(content: &str) -> String {
    let imports = needed_imports(content);
    let mut out = String::with_capacity(content.len() + 64);
    let mut saw_pkg = false;
    let mut imports_emitted = false;
    for line in content.lines() {
        if !saw_pkg && line.starts_with("package ") {
            out.push_str("package v2\n");
            saw_pkg = true;
        } else {
            // First non-package, non-comment line is where the imports
            // go. Pre-pending here keeps them above transpiled headers.
            if saw_pkg && !imports_emitted && !imports.is_empty()
                && !line.starts_with("//")
                && !line.trim().is_empty()
            {
                emit_imports(&mut out, &imports);
                imports_emitted = true;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    // Fallthrough: file had only comments + package line. Still emit
    // imports if needed (the body might be empty, but that's harmless).
    if saw_pkg && !imports_emitted && !imports.is_empty() {
        emit_imports(&mut out, &imports);
    }
    if !saw_pkg {
        let mut prefixed = String::from("package v2\n\n");
        if !imports.is_empty() {
            emit_imports(&mut prefixed, &imports);
        }
        prefixed.push_str(&out);
        return prefixed;
    }
    out
}

fn needed_imports(content: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (probe, name) in [
        ("cmp.", "cmp"),
        ("fmt.", "fmt"),
        ("regexp.", "regexp"),
        ("strings.", "strings"),
    ] {
        if content.contains(probe) {
            out.push(name);
        }
    }
    out
}

fn emit_imports(out: &mut String, imports: &[&str]) {
    if imports.len() == 1 {
        out.push_str(&format!("import {:?}\n\n", imports[0]));
    } else {
        out.push_str("import (\n");
        for name in imports {
            out.push_str(&format!("\t{name:?}\n"));
        }
        out.push_str(")\n\n");
    }
}

// Re-export the library emit functions used by `runtime_loader::GO_TARGET`.
pub(crate) use library::{emit_library_class, emit_module, format_constant, format_module_ivar};
