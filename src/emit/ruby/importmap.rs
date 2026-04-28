//! `config/importmap.rb` emission. Two modes:
//!
//! - `emit_importmap`: round-trip Rails-shape — emits `pin "name"` /
//!   `pin "name", to: "path"` DSL calls, the same form Rails parses.
//!
//! - `emit_lowered_importmap`: spinel-shape — emits a frozen
//!   `Importmap::PINS` array of `{name:, path:}` hashes plus an
//!   `ENTRY` constant. Mirrors `src/importmap.rs::PINS` on the Rust
//!   target. The runtime's `ViewHelpers.javascript_importmap_tags`
//!   reads PINS to render modulepreloads + the importmap script's
//!   import map JSON. No DSL parsing at runtime; the lowerer is the
//!   one that ingested `config/importmap.rb`.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;

pub(super) fn emit_importmap(importmap: &crate::app::Importmap) -> EmittedFile {
    let mut s = String::new();
    for pin in &importmap.pins {
        // Reconstruct the `to:` kwarg when the path deviates from
        // the default `/assets/<name>.js` derivation. Keeps the
        // common case terse.
        let default_path = format!("/assets/{}.js", pin.name);
        if pin.path == default_path {
            writeln!(s, "pin {:?}", pin.name).unwrap();
        } else {
            // Strip leading `/assets/` so the emitted `to:` uses
            // the same shorthand Rails does.
            let to = pin
                .path
                .strip_prefix("/assets/")
                .unwrap_or(&pin.path);
            writeln!(s, "pin {:?}, to: {:?}", pin.name, to).unwrap();
        }
    }
    EmittedFile {
        path: PathBuf::from("config/importmap.rb"),
        content: s,
    }
}

/// Spinel-shape importmap: emits a frozen array of `{name:, path:}`
/// hashes plus the entry-point name as constants under `Importmap`.
/// The runtime's `ViewHelpers.javascript_importmap_tags(pins, entry)`
/// reads these to render modulepreloads + the `<script type="importmap">`
/// JSON. Mirrors `src/emit/rust/importmap.rs::PINS`.
pub(super) fn emit_lowered_importmap(app: &crate::App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "module Importmap").unwrap();
    writeln!(s, "  PINS = [").unwrap();
    if let Some(importmap) = &app.importmap {
        for pin in &importmap.pins {
            writeln!(
                s,
                "    {{ name: {:?}, path: {:?} }},",
                pin.name, pin.path
            )
            .unwrap();
        }
    }
    writeln!(s, "  ].freeze").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  ENTRY = \"application\".freeze").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("config/importmap.rb"),
        content: s,
    }
}
