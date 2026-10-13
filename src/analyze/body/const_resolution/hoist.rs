//! Singleton-segment stripping and nested-namespace indexing for
//! [`super::ConstResolver`].
//!
//! Kept beside the resolver so the public facade file stays under the
//! ~1k-line bar while these helpers grow with Rubydex naming quirks.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ty::Ty;

/// The name a declaration answers to once ingest has hoisted it.
///
/// Rubydex names what `class << self` declares by its singleton class:
/// `Gate::<Gate>::ERR`. Every Roundhouse walk replays such a
/// constant in the enclosing body (`singleton_class::SingletonBody::
/// class_body`, `library_class::walk_decl_body`), so emitted code defines
/// it as `Gate::ERR`. Qualifying a reference with the singleton
/// segment emitted `Gate::<Gate>::ERR`, which is not Ruby.
pub(super) fn hoisted_name(name: &str) -> std::borrow::Cow<'_, str> {
    if !name.contains('<') {
        return std::borrow::Cow::Borrowed(name);
    }
    std::borrow::Cow::Owned(
        name.split("::")
            .filter(|segment| !segment.starts_with('<'))
            .collect::<Vec<_>>()
            .join("::"),
    )
}

/// Map each source namespace to the classes/modules declared inside it
/// (`A` → `A::B`, `A::B::C`).
pub(super) fn nested_namespaces_index(declared: &[&str]) -> HashMap<Box<str>, Vec<Box<str>>> {
    let mut nested_namespaces: HashMap<Box<str>, Vec<Box<str>>> = HashMap::new();
    for name in declared {
        for (at, _) in name.match_indices("::") {
            if declared.binary_search(&&name[..at]).is_ok() {
                nested_namespaces
                    .entry(Box::from(&name[..at]))
                    .or_default()
                    .push(Box::from(*name));
            }
        }
    }
    nested_namespaces
}

/// Literal runtime values come from the same embedded implementations
/// the name graph indexes. Keep their full owners, not suffix aliases,
/// and share types between references instead of copying their trees.
pub(super) fn runtime_value_types() -> &'static HashMap<String, Arc<Ty>> {
    static VALUES: std::sync::OnceLock<HashMap<String, Arc<Ty>>> = std::sync::OnceLock::new();
    VALUES.get_or_init(|| {
        let mut values: HashMap<String, Arc<Ty>> = HashMap::new();
        let mut ambiguous = std::collections::HashSet::new();
        for (_, text) in crate::runtime_files::ruby_sources() {
            let (_, owners) = crate::runtime_src::parse_module_constant_tables(text, true);
            for (owner, constants) in owners {
                for (name, ty) in constants {
                    let name = format!("{}::{}", owner.0.as_str(), name.as_str());
                    if ambiguous.contains(&name) {
                        continue;
                    }
                    if values.get(&name).is_some_and(|previous| **previous != ty) {
                        values.remove(&name);
                        ambiguous.insert(name);
                    } else {
                        values.insert(name, Arc::new(ty));
                    }
                }
            }
        }
        values
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hoisted_name_strips_singleton_segments() {
        assert_eq!(hoisted_name("Gate::ERR").as_ref(), "Gate::ERR");
        assert_eq!(hoisted_name("Gate::<Gate>::ERR").as_ref(), "Gate::ERR");
    }

    #[test]
    fn nested_index_lists_inner_namespaces() {
        let declared = ["A", "A::B", "A::B::C", "Z"];
        let index = nested_namespaces_index(&declared);
        let a = index.get("A").expect("A");
        assert!(a.iter().any(|n| n.as_ref() == "A::B"));
        assert!(a.iter().any(|n| n.as_ref() == "A::B::C"));
        assert!(!index.contains_key("Z"));
    }
}
