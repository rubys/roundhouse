//! Order-independent static inputs for Rails enum declarations. Source
//! collection and Sorbet invalidation are separate from lexical resolution.
//! Invalid inputs still shadow outer names; members use canonical metadata.

mod context;
mod policy;
mod source;

use ruby_prism::Node;

use super::super::util::constant_id_str;
use crate::Symbol;
use crate::expr::Literal;

/// One entry per declared name. Unsupported values still shadow outer
/// namespaces; Sorbet members keep their names for write validation.
enum EnumConstant {
    Namespace,
    Mapping(Vec<(String, Literal)>),
    Sorbet {
        members: Vec<(Symbol, Literal)>,
        base: String,
        owners: Vec<String>,
    },
    Unsupported,
}

/// Syntax roles, not safety decisions. Only policy interprets these roles.
enum ValueContext {
    Discarded,
    Receiver,
    SerializedPair,
    Argument {
        receiver: Option<String>,
        method: String,
    },
    Stored,
    Returned,
    Chained,
    Unmodeled,
}

enum SurfaceUse {
    Reference(ValueContext),
    Call {
        method: String,
        result: ValueContext,
    },
    EnumBase,
    Inheritance,
    Override,
}

struct ScopedName {
    path: String,
    owners: Vec<String>,
}

enum SourceFact {
    Write(ScopedName),
    Use {
        subject: ScopedName,
        context: SurfaceUse,
    },
    Reopen {
        subject: ScopedName,
        collected: String,
    },
}

#[derive(Default)]
pub(in crate::ingest) struct EnumConstants {
    values: std::collections::HashMap<String, EnumConstant>,
    pub(super) nesting: std::collections::HashMap<(String, usize), Vec<String>>,
    /// Superclass identities are lookup barriers, never additional enum inputs.
    parents: std::collections::HashMap<String, Vec<ScopedName>>,
    facts: Vec<SourceFact>,
}

impl EnumConstants {
    /// Only already-consumed original Ruby can add mutation facts here.
    /// Declaration admission remains the prepass's input roots. Revalidate
    /// enum declarations only: no model replacement or unrelated diagnostics.
    pub(in crate::ingest) fn validate_consumed_sources(
        &mut self,
        app: &crate::App,
        input_files: &std::collections::HashSet<std::path::PathBuf>,
    ) -> super::super::IngestResult<()> {
        for source in &app.sources {
            let path = std::path::Path::new(&source.path);
            if path.extension().is_some_and(|extension| extension == "rb")
                && !input_files.contains(path)
            {
                self.record_mutations(source.text.as_bytes(), &source.path);
            }
        }
        self.finish();
        for model in app.models.iter().filter(|model| !model.enums.is_empty()) {
            if let Some(source) = model.span.file.0.checked_sub(1)
                .and_then(|index| app.sources.get(index as usize))
            {
                super::validate_sorbet_enum_mappings(model, source, self)?;
            }
        }
        Ok(())
    }

    fn qualify(owner: &str, name: &str) -> String {
        if let Some(rooted) = name.strip_prefix("::") {
            rooted.to_string()
        } else if owner.is_empty() {
            name.to_string()
        } else {
            format!("{owner}::{name}")
        }
    }

    fn resolve_name(&self, path: &str, owners: &[String]) -> Option<String> {
        self.resolve_name_in(path, owners, &mut std::collections::HashSet::new())
    }

    fn resolve_name_in(
        &self,
        path: &str,
        owners: &[String],
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<String> {
        if let Some(rooted) = path.strip_prefix("::") {
            return Some(rooted.to_string());
        }
        let first = path.split("::").next()?;
        for owner in owners {
            if self.values.contains_key(&Self::qualify(owner, first)) {
                return Some(Self::qualify(owner, path));
            }
        }
        // Ruby searches lexical owners, then the innermost class's ancestry,
        // then Object. Do not fold inherited values, or fall past their shadow.
        if owners.first().is_some_and(|owner| self.inherited_shadow(owner, first, visited)) {
            return None;
        }
        if self.values.contains_key(first) {
            return Some(path.to_string());
        }
        // The one external namespace assumed here is Sorbet's gem base.
        // Its qualified writes/overrides still invalidate dependent enums.
        matches!(path, "T" | "T::Enum").then(|| path.to_string())
    }

    fn inherited_shadow(
        &self,
        owner: &str,
        first: &str,
        visited: &mut std::collections::HashSet<String>,
    ) -> bool {
        if !visited.insert(owner.to_string()) {
            return true; // Cyclic ancestry is not a provable root lookup.
        }
        let shadow = self.parents.get(owner).is_some_and(|parents| parents.iter().any(|parent| {
            self.resolve_name_in(&parent.path, &parent.owners, visited).is_some_and(|base| {
                self.values.contains_key(&Self::qualify(&base, first))
                    || self.inherited_shadow(&base, first, visited)
            })
        }));
        visited.remove(owner);
        shadow
    }

    fn namespace_chain_valid(&self, name: &str) -> bool {
        let mut namespace = name;
        while let Some((parent, _)) = namespace.rsplit_once("::") {
            if !matches!(self.values.get(parent), Some(EnumConstant::Namespace)) {
                return false;
            }
            namespace = parent;
        }
        true
    }

    pub(super) fn resolve(
        &self,
        node: &Node<'_>,
        owners: &[String],
    ) -> Option<Vec<(String, Literal)>> {
        let values_receiver = node.as_call_node().and_then(|call| {
            (constant_id_str(&call.name()) == "values"
                && call.arguments().is_none()
                && call.block().is_none())
            .then(|| call.receiver())
            .flatten()
        });
        let input = values_receiver.as_ref().unwrap_or(node);
        // Resolve the FIRST segment lexically, then read the rest strictly
        // through that owner. Never fall back past a shadowing namespace.
        let name = self.resolve_name(&Self::path(input)?, owners)?;
        if !self.namespace_chain_valid(&name) {
            return None;
        }
        match self.values.get(&name)? {
            EnumConstant::Mapping(mapping) if values_receiver.is_none() => Some(mapping.clone()),
            EnumConstant::Sorbet { members, .. } if values_receiver.is_some() => members
                .iter()
                .map(|(_, value)| match value {
                    Literal::Str { value: label } => Some((label.clone(), value.clone())),
                    _ => None,
                })
                .collect(),
            _ => None,
        }
    }
}
