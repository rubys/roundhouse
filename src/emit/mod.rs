//! IR → target language emitters.
//!
//! Each emitter takes an `&App` and produces a set of files (`EmittedFile`s).
//! Emitters are pure: no I/O, no filesystem — the caller decides where to write.

pub mod crystal;
pub mod elixir;
pub mod eq;
pub mod go;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod typescript;

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedFile {
    pub path: PathBuf,
    pub content: String,
}
