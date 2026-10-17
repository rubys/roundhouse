//! IR → target language emitters.
//!
//! Each emitter takes an `&App` and produces a set of files (`EmittedFile`s).
//! Emitters are pure: no I/O, no filesystem — the caller decides where to write.

pub mod crystal;
pub mod csharp;
pub mod diagnostics;
pub mod elixir;
pub mod futamura;
pub mod go;
pub mod kotlin;
pub mod python;
pub mod roda;
pub mod ruby;
pub mod rust;
pub mod shared;
pub mod swift;
pub mod typescript;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::dialect::LibraryClass;

/// Unique class leaves in one file, spelled by `spell`. A leaf shared
/// by two classes, or already equal to that spelling, is omitted.
/// Go spells `Owner::Leaf` as `OwnerLeaf`; Crystal keeps `Owner::Leaf`.
pub fn unique_leaf_names(
    classes: &[LibraryClass],
    spell: impl Fn(&str) -> String,
) -> HashMap<String, String> {
    let mut by_leaf: HashMap<String, Vec<String>> = HashMap::new();
    for class in classes {
        let raw = class.name.0.as_str();
        let leaf = raw.rsplit("::").next().unwrap_or(raw).to_string();
        by_leaf.entry(leaf).or_default().push(spell(raw));
    }
    by_leaf
        .into_iter()
        .filter_map(|(leaf, full)| {
            let [only] = full.as_slice() else { return None };
            if only == &leaf {
                None
            } else {
                Some((leaf, only.clone()))
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedFile {
    pub path: PathBuf,
    pub content: String,
}
