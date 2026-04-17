use serde::{Deserialize, Serialize};
use std::fmt;

/// A textual name (methods, variables, types). Newtype so the internal
/// representation can switch to an interned form later without breaking consumers.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Symbol(String);

impl Symbol {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol(s.to_string())
    }
}

impl From<String> for Symbol {
    fn from(s: String) -> Self {
        Symbol(s)
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
