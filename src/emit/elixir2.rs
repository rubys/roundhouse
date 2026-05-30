//! Elixir target — `elixir2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `go2` / `rust2` migration pattern. Strangler-fig
//! overlay that runs alongside the legacy `src/emit/elixir.rs` while
//! the migration to the lowered IR (`LibraryClass` + `MethodDef`,
//! transpiled from `runtime/ruby/`) lands.
//!
//! The overlay emits transpiled framework-runtime files under
//! `lib/v2/` inside a dedicated `V2.*` Elixir module namespace, so it
//! can never collide with the legacy hand-written `runtime/elixir/*.ex`
//! (which live under `Roundhouse.*`) or with legacy app-emitted modules
//! (`Router`, `Post`, …). The legacy emit is otherwise untouched: it
//! still produces every `lib/*.ex` it always has.
//!
//! Phase 1 scope: scaffold + minimal transpile of the narrowest runtime
//! slice (`inflector.rb` only). Method bodies are `raise "elixir2 stub"`
//! — `mix compile --warnings-as-errors` over `lib/v2/` produces a real
//! error inventory we can drive future sessions against. The runtime
//! table (`ELIXIR_RUNTIME` in `runtime_loader.rs`) widens one file at a
//! time as the per-variant body walker in `expr.rs` grows coverage.
//!
//! Why Elixir is the high-information target: it's functional and
//! immutable, so the lowered IR's mutable-receiver assumptions
//! (`self.foo = x`, in-place `save`) can't translate directly —
//! mutation must be threaded through return values at call sites. That
//! problem doesn't bite at Phase 1 (bodies are stubs); the inventory is
//! what will quantify where it actually matters.
//!
//! Deferred to later phases: `ty.rs` (Elixir is dynamically typed —
//! Phase 1 emits no type info), models / controllers / views (those
//! stay on the legacy emitter for now), and the hand-written module-
//! state holder for `ActionView::ViewHelpers`'s `@slots`.

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod paths;

use paths::{output_path, OutputKind};

// Re-export the library emit functions consumed by
// `runtime_loader::ELIXIR_TARGET`.
pub use library::{emit_library_class, emit_module, format_constant};

/// Append the elixir2 transpiled-runtime overlay to `files`.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
    files.extend(emit_overlay_files(app));
}

/// Produce the elixir2 overlay files. Phase 1: just the transpiled
/// framework runtime (`lib/v2/inflector.ex`, …), emitted
/// unconditionally — the slice has no app-dependent shims yet.
pub fn emit_overlay_files(_app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();

    let units = match crate::runtime_loader::elixir_units(|_ns, classes| classes) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather than
            // a panic — `mix compile` picks it up and the overlay test
            // sees a non-empty (failing) result. Mirrors go2.
            out.push(EmittedFile {
                path: output_path(OutputKind::TranspileError).path,
                content: format!("# elixir2 transpile failed: {e}\n"),
            });
            return out;
        }
    };

    for unit in &units {
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.ex".to_string());
        let dest = output_path(OutputKind::TranspiledRuntime { file_name: &file_name });
        out.push(EmittedFile {
            path: dest.path,
            content: unit.content.clone(),
        });
    }

    out
}
