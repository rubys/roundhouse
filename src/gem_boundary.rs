//! The typed boundary to gems the analyzer does not otherwise model.
//!
//! A gem's public surface is not in the app's tree, but it is often on
//! disk anyway: Tapioca writes one RBI per locked gem under
//! `sorbet/rbi/gems/`. Those files are declarations only -- classes,
//! their ancestry, and `sig`-typed method headers -- which is exactly
//! what a boundary needs and nothing more. This is the table
//! [`crate::ingest::rbi`] reads them into.
//!
//! It is kept apart from `App::rbs_signatures` on purpose. That table
//! is the app's OWN sidecar (`sig/**/*.rbs`) and every consumer of it,
//! the RBS emitters included, treats its entries as things the app
//! declares. A gem's methods are not the app's to declare, so they are
//! carried here and only the analyzer's dispatch registry reads them.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Ruby's own top-level names. An RBI that mentions them is patching or
/// extending the language, not declaring a gem class.
pub(crate) const RUBY_CORE: &[&str] = &[
    "BasicObject", "Object", "Module", "Class", "Kernel", "Comparable", "Enumerable", "Enumerator",
    "Numeric", "Integer", "Float", "Rational", "Complex", "String", "Symbol", "Array", "Hash",
    "Range", "Regexp", "MatchData", "Struct", "Proc", "Method", "UnboundMethod", "Time", "Date",
    "DateTime", "IO", "File", "Dir", "Exception", "StandardError", "RuntimeError", "NilClass",
    "TrueClass", "FalseClass", "Set", "Random", "Process", "Signal", "Math", "Marshal", "GC",
    "ObjectSpace", "Thread", "Mutex", "Fiber", "Ractor", "Data", "Encoding", "Binding",
    "BigDecimal", "Pathname", "StringIO", "URI", "JSON", "Logger", "Singleton", "Forwardable",
];

/// Every gem class or module an RBI declares, by qualified name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GemBoundary {
    pub classes: HashMap<ClassId, GemClass>,
    /// For each gem whose RBI was read: the qualified names of the
    /// classes and modules it actually defines something in (methods,
    /// constants, ancestry). A gem in here has told us exactly what it
    /// owns; a bare namespace container (`module Shopify` opened only
    /// to nest a class) owns nothing beside what it nests.
    #[serde(default)]
    pub namespaces: HashMap<String, BTreeSet<String>>,
    /// Locked gems required at boot by this app's extensions. An RBI or
    /// a dependency entry alone does not establish that a gem is loaded.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub loaded: BTreeSet<String>,
}

impl GemBoundary {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.loaded.is_empty()
    }

    /// Whether `gem`'s RBI accounts for the constant `path`: it defines
    /// it, defines something inside it, or `path` sits inside something
    /// it defines. `None` when its RBI was not read: nothing is known
    /// either way.
    pub fn declares_path(&self, gem: &str, path: &str) -> Option<bool> {
        let owned = self.namespaces.get(gem)?;
        let inside = format!("{path}::");
        if owned.contains(path) || owned.range(inside.clone()..).next().is_some_and(|n| n.starts_with(&inside)) {
            return Some(true);
        }
        Some(path.match_indices("::").any(|(i, _)| owned.contains(&path[..i])))
    }
}

/// One class or module as an RBI declares it.
///
/// Class names inside `Ty`s are as WRITTEN: resolving them needs the
/// registry of every class the analysis knows, which only exists once
/// the analyzer is built.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GemClass {
    /// The gem the declaration came from (a lockfile spec name).
    pub gem: String,
    pub is_module: bool,
    /// The superclass as written, `None` when the RBI names none.
    pub parent: Option<ClassId>,
    /// `include` / `prepend` targets, as written.
    pub includes: Vec<ClassId>,
    /// `extend` targets, as written: their instance methods are this
    /// class's class-side surface.
    pub extends: Vec<ClassId>,
    pub instance_methods: HashMap<Symbol, Ty>,
    pub class_methods: HashMap<Symbol, Ty>,
    /// Constants the body assigns (`DEFAULT = ...`), which are values,
    /// not classes.
    pub constants: Vec<Symbol>,
    /// The surface cannot be enumerated from the declaration: the class
    /// answers `method_missing`, or its superclass is an expression
    /// rather than a name. A lookup that misses is then unknown, not
    /// wrong.
    pub dynamic: bool,
}
