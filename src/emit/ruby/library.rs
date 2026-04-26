//! Library-shape Ruby emission — for transpiled-shape input where class
//! bodies already contain explicit methods (no Rails DSL expansion).
//! Mirrors `src/emit/typescript/library.rs` in scope; produces one
//! `app/models/<name>.rb` per `LibraryClass`.
//!
//! Ruby is implicit about ivar declaration and global about constant
//! resolution, so this emitter is shorter than the TS analog: no ivar
//! field block, no import partition.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use crate::App;
use crate::dialect::LibraryClass;
use crate::naming::snake_case;

pub(super) fn emit_library_class_decls(app: &App) -> Vec<EmittedFile> {
    app.library_classes
        .iter()
        .map(emit_library_class_decl)
        .collect()
}

fn emit_library_class_decl(lc: &LibraryClass) -> EmittedFile {
    let name = lc.name.0.as_str();
    let file_stem = snake_case(name);
    let mut s = String::new();

    let header = match lc.parent.as_ref() {
        Some(p) => format!("class {name} < {}", p.0.as_str()),
        None => format!("class {name}"),
    };
    writeln!(s, "{header}").unwrap();

    for inc in &lc.includes {
        writeln!(s, "  include {}", inc.0.as_str()).unwrap();
    }
    if !lc.includes.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    let mut first = true;
    for m in &lc.methods {
        if !first {
            writeln!(s).unwrap();
        }
        first = false;
        let body = super::emit_method(m);
        for line in body.lines() {
            if line.is_empty() {
                writeln!(s).unwrap();
            } else {
                writeln!(s, "  {line}").unwrap();
            }
        }
    }

    writeln!(s, "end").unwrap();

    EmittedFile {
        path: PathBuf::from(format!("app/models/{file_stem}.rb")),
        content: s,
    }
}
