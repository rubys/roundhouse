//! `config/importmap.rb` round-trip emission — emits `pin "name"` /
//! `pin "name", to: "path"` DSL calls, the same form Rails parses.
//! The lowered/spinel-shape variant retired 2026-05-01: superseded
//! by `library::emit_module_file` consuming the universal
//! `lower_importmap_to_library_functions` output.

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

// emit_lowered_importmap retired 2026-05-01 — superseded by the
// universal `library::emit_module_file` consuming
// `lower_importmap_to_library_functions`. The previous shape
// (`Importmap::PINS` constant + `Importmap::ENTRY` constant) was
// reconciled to the more general method-based form
// (`Importmap.pins`, `Importmap.entry`) shared with TS.
