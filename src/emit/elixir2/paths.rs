//! Output-path routing for the elixir2 overlay.
//!
//! Mirrors `src/emit/go2/paths.rs`: every emitted file flows through
//! `output_path`, so a future per-package cutover is a focused diff
//! against this one function rather than scattered path literals.
//!
//! Phase 1 invariant: transpiled runtime lands under `lib/v2/<name>.ex`.

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

    /// A transpiled per-controller app module (`V2.<Name>Controller`),
    /// produced from the lowered controller `LibraryClass`. Filename is
    /// the snake-cased module name (`articles_controller.ex`).
    Controller { file_name: &'a str },

    /// The generated route table (`V2.RoutesTable`) — a list of
    /// `V2.ActionDispatch.Router.Route` structs the stateless router
    /// `match/3` consumes.
    RoutesTable,

    /// The generated dispatch shim (`V2.Dispatch`) — one `case` arm per
    /// app controller, constructing the controller struct and running
    /// `process_action`.
    Dispatch,

    /// The generated boot entry (`V2.Main.run/0`).
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
        TranspiledRuntime { file_name } => format!("lib/v2/{file_name}"),
        HandWrittenRuntime { name } => format!("lib/v2/{name}"),
        Controller { file_name } => format!("lib/v2/{file_name}"),
        RoutesTable => "lib/v2/routes_table.ex".to_string(),
        Dispatch => "lib/v2/dispatch.ex".to_string(),
        Main => "lib/v2/main.ex".to_string(),
        TranspileError => "lib/v2/transpile_error.txt".to_string(),
    };
    OutputDest {
        path: PathBuf::from(path),
    }
}
