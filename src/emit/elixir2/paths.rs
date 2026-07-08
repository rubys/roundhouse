//! Output-path routing for the elixir2 overlay.
//!
//! Mirrors `src/emit/go2/paths.rs`: every emitted file flows through
//! `output_path`, so a future per-package cutover is a focused diff
//! against this one function rather than scattered path literals.
//!
//! Phase 1 invariant: transpiled runtime lands under `lib/<name>.ex`.

use std::path::PathBuf;

/// Logical kind of an emitted elixir2 file.
pub(crate) enum OutputKind<'a> {
    /// Transpiled framework-runtime module produced by
    /// `runtime_loader::elixir_units` (e.g. `inflector.ex`).
    TranspiledRuntime { file_name: &'a str },

    /// Hand-written runtime module copied verbatim (e.g. `db.ex` — the
    /// portable prepared-cursor surface the lowered model emit targets,
    /// with no useful Ruby source to transpile). Mirrors go2's
    /// `HandWrittenRuntime`.
    HandWrittenRuntime { name: &'a str },

    /// A transpiled per-controller app module (`<Name>Controller`),
    /// produced from the lowered controller `LibraryClass`. Filename is
    /// the snake-cased module name (`articles_controller.ex`).
    // Constructed by the gated Phase C controller-emit path (elixir2
    // app-shell), which is not yet committed on main.
    #[allow(dead_code)]
    Controller { file_name: &'a str },

    /// The generated route table (`RoutesTable`) — a list of
    /// `ActionDispatch.Router.Route` structs the stateless router
    /// `match/3` consumes.
    RoutesTable,

    /// The generated dispatch shim (`Dispatch`) — one `case` arm per
    /// app controller, constructing the controller struct and running
    /// `process_action`.
    Dispatch,

    /// The generated boot entry (`Main.run/0`).
    Main,

    /// Sentinel emitted when transpile fails — picked up by
    /// `mix compile` so the failure surfaces as a real build error.
    TranspileError,
}

/// Output destination for a file emitted by the elixir2 overlay.
pub(crate) struct OutputDest {
    /// Filesystem path (relative to the emit root).
    pub path: PathBuf,
}

/// Resolve an `OutputKind` to its emitted path.
pub(crate) fn output_path(kind: OutputKind<'_>) -> OutputDest {
    use OutputKind::*;
    let path = match kind {
        TranspiledRuntime { file_name } => format!("lib/{file_name}"),
        HandWrittenRuntime { name } => format!("lib/{name}"),
        Controller { file_name } => format!("lib/{file_name}"),
        RoutesTable => "lib/routes_table.ex".to_string(),
        Dispatch => "lib/dispatch.ex".to_string(),
        Main => "lib/main.ex".to_string(),
        TranspileError => "lib/transpile_error.txt".to_string(),
    };
    OutputDest {
        path: PathBuf::from(path),
    }
}
