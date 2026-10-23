//! Native indexer overrides share their ancestor's key contract.

use std::collections::{HashMap, HashSet};

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::ty::Ty;

#[derive(Default)]
pub(crate) struct IndexerKeys {
    classes: HashMap<String, (Option<String>, Option<Ty>)>,
}

impl IndexerKeys {
    pub(crate) fn register(&mut self, classes: &[LibraryClass]) {
        for class in classes {
            let key = ["[]", "[]="].iter().find_map(|name| {
                let method = class
                    .methods
                    .iter()
                    .find(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == *name)?;
                match method.signature.as_ref() {
                    Some(Ty::Fn { params, .. }) => params.first().map(|p| (*p.ty).clone()),
                    _ => Some(Ty::Untyped),
                }
            });
            self.classes.insert(
                class.name.0.to_string(),
                (class.parent.as_ref().map(|p| p.0.to_string()), key),
            );
        }
    }

    pub(crate) fn inherited(&self, class: &str) -> Option<&Ty> {
        self.classes.get(self.inherited_class(class)?)?.1.as_ref()
    }

    /// The ancestor that establishes the key contract, through helper overrides.
    pub(crate) fn inherited_class(&self, class: &str) -> Option<&str> {
        let mut current = self.classes.get(class)?.0.as_deref();
        let mut seen = HashSet::new();
        let mut ancestor = None;
        while let Some(name) = current {
            if !seen.insert(name) {
                break;
            }
            let Some((parent, own)) = self.classes.get(name) else {
                break;
            };
            // An intermediate override itself uses its ancestor's contract.
            if own.is_some() {
                ancestor = Some(name);
            }
            current = parent.as_deref();
        }
        ancestor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_follow_the_root_key_through_intermediate_classes() {
        let mut keys = IndexerKeys::default();
        for root in [
            Ty::Str,
            Ty::Int,
            Ty::Union {
                variants: vec![Ty::Str, Ty::Sym].into(),
            },
        ] {
            keys.classes
                .insert("Base".into(), (None, Some(root.clone())));
            keys.classes
                .insert("Middle".into(), (Some("Base".into()), Some(Ty::Untyped)));
            keys.classes
                .insert("Child".into(), (Some("Middle".into()), Some(Ty::Sym)));
            keys.classes
                .insert("Leaf".into(), (Some("Child".into()), None));
            assert_eq!(keys.inherited("Child"), Some(&root));
            assert_eq!(keys.inherited("Leaf"), Some(&root));
            assert_eq!(keys.inherited("Base"), None);
        }
    }

    #[test]
    fn registered_helpers_keep_qualified_and_concrete_ancestor_keys() {
        for spelling in ["String", "Integer", "String | Symbol", "Symbol | String"] {
            let source = br#"module Helpers
  class Root
    def [](key)
      nil
    end
    def []=(key, value)
      nil
    end
  end
  class Child < Helpers::Root
    def [](key)
      nil
    end
  end
end
"#;
            let rbs = format!(r#"module Helpers
  class Root
    def []: (({spelling}) key) -> untyped
    def []=: (({spelling}) key, untyped value) -> untyped
  end
  class Child < Helpers::Root
    def []: (untyped key) -> untyped
  end
end
"#);
            let mut classes = crate::runtime_src::parse_library_with_rbs(source, &rbs, "helpers.rb").unwrap();
            classes.reverse();
            let mut keys = IndexerKeys::default();
            keys.register(&classes);
            let expected = keys.classes["Helpers::Root"].1.as_ref();
            assert!(expected.is_some(), "{spelling}");
            assert_eq!(keys.inherited("Helpers::Child"), expected, "{spelling}");
        }
    }
}
