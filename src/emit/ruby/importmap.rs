//! `config/importmap.rb` emission. Round-trips the ingested
//! name→path shape; `pin_all_from` is expanded at ingest time into
//! explicit per-file pins, so this emits explicit `pin` calls even
//! if the source used the shorter `pin_all_from` form. Verbose but
//! semantics-preserving — the file is emitter-generated, not
//! human-maintained.

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
