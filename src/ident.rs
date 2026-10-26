//! Identifier newtypes shared by every IR layer: `Symbol` for names,
//! `ClassId` / `TableRef` for class and table references, and the
//! `VarId` / `TyVar` / `EffectVar` counters the analyzer allocates
//! during inference. Ingest mints the named ones; everything
//! downstream keys its maps and registries by them. A name's text is
//! shared through an `Arc` (see [`Symbol`]), and the newtypes are the
//! point: a class reference can never be confused with a table name or
//! a bare method symbol in a signature, and the representation could
//! change again without touching a single consumer.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A textual name (methods, variables, types). Newtype so the internal
/// representation can change without breaking consumers.
///
/// Held as an `Arc<str>`, so cloning a symbol is a reference-count bump
/// rather than a heap allocation; the analyzer clones names constantly
/// (map keys, lookup tuples, per-scope contexts). Equality, ordering and
/// hashing are by content, exactly as they were for the `String` this
/// replaced, so map iteration order and every emitted name are unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "String")]
pub struct Symbol(Arc<str>);

impl Symbol {
    pub fn new(s: impl AsRef<str>) -> Self {
        Self(Arc::from(s.as_ref()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Eq for Symbol {}

impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (*self.0).hash(state)
    }
}

impl PartialOrd for Symbol {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Symbol {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self.0).cmp(&*other.0)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol(Arc::from(s))
    }
}

impl From<String> for Symbol {
    fn from(s: String) -> Self {
        Symbol(Arc::from(s))
    }
}

/// Locally-unique id for a variable binding.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VarId(pub u32);

/// Type inference variable.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TyVar(pub u32);

/// Effect inference variable.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EffectVar(pub u32);

/// Stable reference to a class by name.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClassId(pub Symbol);

impl fmt::Display for ClassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable reference to a database table by name.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TableRef(pub Symbol);

impl fmt::Display for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
