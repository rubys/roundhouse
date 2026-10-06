//! Invalidate symbolic enum inputs only after all source facts are collected.
//! This policy has no Prism dependencies and never evaluates application Ruby.

use super::{EnumConstant, EnumConstants, SourceFact, SurfaceUse, ValueContext};

impl EnumConstants {
    pub(in crate::ingest) fn finish(&mut self) {
        let facts = std::mem::take(&mut self.facts);
        for fact in &facts {
            let SourceFact::Write(subject) = fact else {
                continue;
            };
            if let Some(name) = self.resolve_name(&subject.path, &subject.owners) {
                // A later qualified member write invalidates its canonical
                // owner; there is no separate assignment-count ledger.
                if let Some((owner, member)) = name.rsplit_once("::") {
                    if matches!(self.values.get(owner), Some(EnumConstant::Sorbet { members, .. })
                        if members.iter().any(|(name, _)| name.as_str() == member))
                    {
                        self.values
                            .insert(owner.to_string(), EnumConstant::Unsupported);
                    }
                }
                self.values.insert(name, EnumConstant::Unsupported);
            }
        }
        // Relative qualified reopens search lexically: `module Wrapper;
        // class Catalog::Rating` may reopen ::Catalog::Rating, not a class
        // beneath Wrapper. Refuse any enum touched through that mismatch.
        // Only enums can be refused, and reopens create none: scan them alone.
        let enums: Vec<String> = self
            .values
            .iter()
            .filter(|(_, v)| matches!(v, EnumConstant::Sorbet { .. }))
            .map(|(name, _)| name.clone())
            .collect();
        for fact in &facts {
            let SourceFact::Reopen { subject, collected } = fact else {
                continue;
            };
            if let Some(resolved) = self.resolve_name(&subject.path, &subject.owners) {
                if resolved == "T::Enum" {
                    self.values.insert(resolved, EnumConstant::Unsupported);
                    continue;
                }
                if resolved != *collected {
                    let prefixes = [format!("{resolved}::"), format!("{collected}::")];
                    for name in enums.iter().filter(|name| {
                        *name == &resolved
                            || *name == collected
                            || prefixes.iter().any(|prefix| name.starts_with(prefix))
                    }) {
                        self.values.insert(name.clone(), EnumConstant::Unsupported);
                    }
                }
            }
        }
        // An argument's consumer may itself be invalidated by a later fact.
        // Revisit all facts until no canonical enum is lost, including base
        // invalidation, rather than granting traversal-order exemptions.
        loop {
            let invalid_before = self
                .values
                .values()
                .filter(|v| matches!(v, EnumConstant::Unsupported))
                .count();
            // A namespace alias invalidates the Sorbet enums beneath it. Scan
            // only the enums (few), once per namespace per round: values gain
            // no new enum during a round, so a repeat use changes nothing.
            let enums: Vec<String> = self
                .values
                .iter()
                .filter(|(_, v)| matches!(v, EnumConstant::Sorbet { .. }))
                .map(|(name, _)| name.clone())
                .collect();
            let mut aliased = std::collections::HashSet::new();
            for fact in &facts {
                let SourceFact::Use { subject, context } = fact else {
                    continue;
                };
                if let Some(name) = self.resolve_name(&subject.path, &subject.owners) {
                    if self.harmless_use(&name, &subject.owners, context) {
                        continue;
                    }
                    if name == "T" || name == "T::Enum" {
                        self.values.insert(name, EnumConstant::Unsupported);
                        continue;
                    }
                    // A namespace alias exposes every enum beneath it, but must
                    // not change the existing literal-array mapping vocabulary.
                    if matches!(self.values.get(&name), Some(EnumConstant::Namespace)) {
                        if aliased.insert(name.clone()) {
                            let prefix = format!("{name}::");
                            for enum_name in enums.iter().filter(|e| e.starts_with(&prefix)) {
                                self.values
                                    .insert(enum_name.clone(), EnumConstant::Unsupported);
                            }
                        }
                        continue;
                    }
                    let owner =
                        if matches!(self.values.get(&name), Some(EnumConstant::Sorbet { .. })) {
                            name.as_str()
                        } else {
                            name.rsplit_once("::").map(|(owner, _)| owner).unwrap_or("")
                        };
                    if matches!(self.values.get(owner), Some(EnumConstant::Sorbet { .. })) {
                        self.values
                            .insert(owner.to_string(), EnumConstant::Unsupported);
                    }
                }
            }
            // A shadowed/replaced T OR T::Enum is not the assumed gem base.
            // Root qualification bypasses lexical shadows, but not replacement
            // of that root base. All declarations/writes are now complete.
            let invalid: Vec<_> = self
                .values
                .iter()
                .filter_map(|(name, value)| {
                    let EnumConstant::Sorbet { base, owners, .. } = value else {
                        return None;
                    };
                    let namespace = base.strip_suffix("::Enum").unwrap();
                    [namespace, base.as_str()]
                        .iter()
                        .any(|path| {
                            self.resolve_name(path, owners)
                                .is_none_or(|name| self.values.contains_key(&name))
                        })
                        .then(|| name.clone())
                })
                .collect();
            for name in invalid {
                self.values.insert(name, EnumConstant::Unsupported);
            }
            if self
                .values
                .values()
                .filter(|v| matches!(v, EnumConstant::Unsupported))
                .count()
                == invalid_before
            {
                break;
            }
        }
        // A later original-source snapshot adds mutation facts. Retain these
        // argument/consumer relationships so invalidation can propagate again.
        self.facts = facts;
    }

    fn harmless_use(&self, name: &str, owners: &[String], usage: &SurfaceUse) -> bool {
        match usage {
            // Inheritance exposes constant objects through the subclass.
            // Only a canonical literal declaration needs the external base.
            SurfaceUse::EnumBase => name == "T::Enum",
            SurfaceUse::Inheritance => false,
            SurfaceUse::Reference(
                ValueContext::Discarded | ValueContext::Receiver | ValueContext::SerializedPair,
            ) => true,
            SurfaceUse::Reference(ValueContext::Argument {
                receiver: Some(receiver),
                method,
            }) => self
                .resolve_name(receiver, owners)
                .is_some_and(|receiver| self.is_sorbet_read(&receiver, method)),
            SurfaceUse::Reference(_) | SurfaceUse::Override => false,
            SurfaceUse::Call { method, result } => {
                let member_owner = name.rsplit_once("::").map(|(owner, _)| owner).unwrap_or("");
                let class_read = name == "T::Enum"
                    || matches!(self.values.get(name), Some(EnumConstant::Sorbet { .. }));
                if class_read
                    || matches!(
                        self.values.get(member_owner),
                        Some(EnumConstant::Sorbet { .. })
                    )
                {
                    self.is_sorbet_read(name, method)
                        && (!class_read
                            || matches!(
                                result,
                                ValueContext::Discarded
                                    | ValueContext::Receiver
                                    | ValueContext::SerializedPair
                            ))
                } else {
                    // No growing reflection/alias denylist: unknown namespace
                    // calls can expose their still-mutable constant children.
                    // Reject them, but leave existing literal-array inputs alone.
                    match method.as_str() {
                        "itself" | "freeze" => {
                            matches!(result, ValueContext::Discarded | ValueContext::Receiver)
                        }
                        "name" | "to_s" | "inspect" | "hash" | "frozen?" | "==" | "eql?"
                        | "equal?" => true,
                        _ => false,
                    }
                }
            }
        }
    }

    fn is_sorbet_read(&self, name: &str, method: &str) -> bool {
        if (name == "T::Enum" && !self.values.contains_key(name) && !self.values.contains_key("T"))
            || (matches!(self.values.get(name), Some(EnumConstant::Sorbet { .. }))
                && self.namespace_chain_valid(name))
        {
            return matches!(method, "values" | "deserialize" | "try_deserialize");
        }
        let Some((owner, member)) = name.rsplit_once("::") else {
            return false;
        };
        self.namespace_chain_valid(owner)
            && matches!(self.values.get(owner), Some(EnumConstant::Sorbet { members, .. })
            if members.iter().any(|(name, _)| name.as_str() == member))
            && matches!(
                method,
                "serialize" | "to_s" | "inspect" | "hash" | "==" | "eql?" | "equal?"
            )
    }
}
