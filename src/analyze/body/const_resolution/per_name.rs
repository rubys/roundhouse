//! Check-only per-name lookup over retained Rubydex namespace facts.
use super::*;
use ruby_prism::Visit;

fn constant_name<'a>(id: &ruby_prism::ConstantId<'a>) -> &'a str {
    std::str::from_utf8(id.as_slice()).unwrap_or("")
}

#[derive(Clone)]
pub(crate) struct Binding {
    pub answer: ResolvedConstant,
    pub name: ClassId,
    pub external: bool,
    pub declaration: DeclarationId,
    pub unverified_loading: bool,
}
pub(crate) enum ConstantLookup<'a> {
    Known {
        binding: &'a Binding,
        original_name: ClassId,
        external: bool,
        unverified_loading: bool,
    },
    ExternalValueNamespace {
        original_name: ClassId,
        unverified_loading: bool,
    },
    Unknown(Symbol),
    Absent,
}
#[derive(Default)]
pub(super) struct Site {
    written: Vec<Symbol>,
    headers: Vec<u32>,
    scopes: Vec<ClassId>,
    unresolved_scope: bool,
}
#[derive(Default)]
pub(super) struct FileNames {
    pub sites: HashMap<u32, Site>,
}
impl FileNames {
    pub fn new(text: &str) -> Self {
        struct Sites {
            names: FileNames,
            headers: Vec<u32>,
        }
        fn path(node: &ruby_prism::Node<'_>) -> Option<Vec<Symbol>> {
            if let Some(n) = node.as_constant_read_node() {
                return Some(vec![Symbol::new(constant_name(&n.name()))]);
            }
            let n = node.as_constant_path_node()?;
            let mut out = match n.parent() {
                Some(p) => path(&p)?,
                None => vec![Symbol::new("")],
            };
            out.push(Symbol::new(constant_name(&n.name()?)));
            Some(out)
        }
        impl<'pr> Visit<'pr> for Sites {
            fn visit_constant_read_node(&mut self, n: &ruby_prism::ConstantReadNode<'pr>) {
                self.names.sites.insert(
                    n.location().end_offset() as u32,
                    Site {
                        written: vec![Symbol::new(constant_name(&n.name()))],
                        headers: self.headers.clone(),
                        ..Default::default()
                    },
                );
            }
            fn visit_constant_path_node(&mut self, n: &ruby_prism::ConstantPathNode<'pr>) {
                if let Some(written) = path(&n.as_node()) {
                    self.names.sites.insert(
                        n.location().end_offset() as u32,
                        Site {
                            written,
                            headers: self.headers.clone(),
                            ..Default::default()
                        },
                    );
                }
                if let Some(parent) = n.parent() {
                    self.visit(&parent);
                }
            }
            fn visit_class_node(&mut self, n: &ruby_prism::ClassNode<'pr>) {
                self.visit(&n.constant_path());
                if let Some(parent) = n.superclass() {
                    self.visit(&parent);
                }
                self.headers
                    .push(n.constant_path().location().end_offset() as u32);
                if let Some(body) = n.body() {
                    self.visit(&body);
                }
                self.headers.pop();
            }
            fn visit_module_node(&mut self, n: &ruby_prism::ModuleNode<'pr>) {
                self.visit(&n.constant_path());
                self.headers
                    .push(n.constant_path().location().end_offset() as u32);
                if let Some(body) = n.body() {
                    self.visit(&body);
                }
                self.headers.pop();
            }
            fn visit_singleton_class_node(&mut self, n: &ruby_prism::SingletonClassNode<'pr>) {
                self.headers.push(u32::MAX);
                if let Some(body) = n.body() {
                    self.visit(&body);
                }
                self.headers.pop();
            }
        }
        let parsed = crate::ingest::prism::parse_silent(text.as_bytes());
        let mut sites = Sites {
            names: Self::default(),
            headers: vec![],
        };
        if parsed.errors().next().is_none() {
            sites.visit(&parsed.node());
        }
        sites.names
    }
    pub fn resolve_scopes(&mut self, owners: &HashMap<u32, ClassId>) {
        for site in self.sites.values_mut() {
            site.scopes = site
                .headers
                .iter()
                .rev()
                .filter_map(|offset| owners.get(offset).cloned())
                .collect();
            site.unresolved_scope = site.scopes.len() != site.headers.len();
        }
    }
}

enum Step {
    Namespace(ClassId),
    Unknown(Symbol),
}
#[derive(Default)]
struct NamespaceFacts {
    members: HashMap<Symbol, Binding>,
    incomplete: bool,
    uncertain_members: HashSet<Symbol>,
    ancestors: Vec<Step>,
}
pub(super) struct NameIndex {
    namespaces: HashMap<ClassId, NamespaceFacts>,
}
enum Search<'a> {
    Known(&'a Binding),
    Unknown(Symbol),
    Absent,
}
fn owner_id(name: &str) -> ClassId {
    ClassId(Symbol::new(if name.is_empty() { "Object" } else { name }))
}
pub(super) struct ReceiverEffect {
    pub owner: Option<ClassId>,
    pub member: Option<Symbol>,
    pub mixin_override: bool,
    pub source: String,
}

/// Effects keep their source nesting until the graph resolves the receiver.
pub(super) fn receiver_effects(graph: &Graph, declarations: &[SourceFile]) -> Vec<ReceiverEffect> {
    struct Effects<'a> {
        answers: &'a FileAnswers,
        source: &'a str,
        scopes: Vec<Option<ClassId>>,
        effects: Vec<ReceiverEffect>,
    }
    impl<'pr> Visit<'pr> for Effects<'_> {
        fn visit_class_node(&mut self, n: &ruby_prism::ClassNode<'pr>) {
            self.scopes.push(
                self.answers
                    .namespace_bodies
                    .get(&(n.constant_path().location().end_offset() as u32))
                    .cloned(),
            );
            if let Some(body) = n.body() {
                self.visit(&body);
            }
            self.scopes.pop();
        }
        fn visit_module_node(&mut self, n: &ruby_prism::ModuleNode<'pr>) {
            self.scopes.push(
                self.answers
                    .namespace_bodies
                    .get(&(n.constant_path().location().end_offset() as u32))
                    .cloned(),
            );
            if let Some(body) = n.body() {
                self.visit(&body);
            }
            self.scopes.pop();
        }
        fn visit_call_node(&mut self, n: &ruby_prism::CallNode<'pr>) {
            use crate::ingest::source_declarations::{
                MIXIN_OVERRIDE, RECEIVER_EFFECT, UNBOUNDED_EFFECT,
            };
            let name = n.name();
            if name.as_slice() == UNBOUNDED_EFFECT.as_bytes() {
                self.effects.push(ReceiverEffect {
                    owner: None,
                    member: None,
                    mixin_override: true,
                    source: self.source.to_string(),
                });
                return;
            }
            if name.as_slice() != RECEIVER_EFFECT.as_bytes()
                && name.as_slice() != MIXIN_OVERRIDE.as_bytes()
            {
                return;
            }
            let owner = match n.receiver() {
                None => self
                    .scopes
                    .last()
                    .cloned()
                    .unwrap_or_else(|| Some(owner_id("Object"))),
                Some(node) if node.as_self_node().is_some() => self
                    .scopes
                    .last()
                    .cloned()
                    .unwrap_or_else(|| Some(owner_id("Object"))),
                Some(node) => {
                    let key = if let Some(path) = node.as_constant_path_node() {
                        Some(path.name_loc().start_offset() as u32)
                    } else {
                        node.as_constant_read_node()
                            .map(|_| node.location().start_offset() as u32)
                    };
                    key.and_then(|key| self.answers.references.get(&key))
                        .and_then(|a| a.as_ref())
                        .and_then(|a| match a {
                            ResolvedConstant::Namespace { class, .. } => Some((**class).clone()),
                            _ => None,
                        })
                }
            };
            // Instance-side contract changes on Module/Class affect an
            // unbounded set of namespace receivers. A singleton override
            // there is conservatively covered by the same barrier.
            let owner = if name.as_slice() == MIXIN_OVERRIDE.as_bytes()
                && owner
                    .as_ref()
                    .is_some_and(|id| matches!(id.0.as_str(), "Module" | "Class"))
            {
                None
            } else {
                owner
            };
            let member = n
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| n.as_string_node())
                .map(|n| Symbol::new(std::str::from_utf8(n.unescaped()).unwrap()));
            self.effects.push(ReceiverEffect {
                owner,
                member,
                mixin_override: name.as_slice() == MIXIN_OVERRIDE.as_bytes(),
                source: self.source.to_string(),
            });
        }
    }
    let mut all = Vec::new();
    for source in declarations {
        let file = SourceFile {
            path: format!("{EXTERNAL_URI_PREFIX}{}", source.path),
            text: source.text.clone(),
        };
        let answers = answer_file(graph, &file, &mut IdentityHashMap::default(), false);
        let parsed = crate::ingest::prism::parse_silent(file.text.as_bytes());
        let mut effects = Effects {
            answers: &answers,
            source: &source.path,
            scopes: Vec::new(),
            effects: Vec::new(),
        };
        effects.visit(&parsed.node());
        all.extend(effects.effects);
    }
    all
}

impl NameIndex {
    pub fn new(graph: &Graph, declarations: &[SourceFile]) -> Self {
        let mut namespaces = HashMap::default();
        let mut cache = IdentityHashMap::default();
        for (id, declaration) in graph.declarations() {
            let Some(ns) = declaration.as_namespace() else {
                continue;
            };
            // Eigenclasses share a hoisted spelling with their attached
            // namespace, but have a different lookup chain.
            if declaration.kind() == "SingletonClass" {
                continue;
            }
            let owner = owner_id(hoisted_name(declaration.name()).as_ref());
            let mut facts = NamespaceFacts::default();
            for id in declaration.definitions() {
                let mixins = match graph.definitions().get(id) {
                    Some(Definition::Class(d)) => d.mixins(),
                    Some(Definition::Module(d)) => d.mixins(),
                    _ => &[],
                };
                for mixin in mixins {
                    let effect = graph
                        .constant_references()
                        .get(mixin.constant_reference_id())
                        .and_then(|r| graph.names().get(r.name_id()))
                        .and_then(|n| graph.strings().get(n.str()))
                        .and_then(|name| {
                            crate::ingest::source_declarations::constant_effect(name.as_str())
                        });
                    match effect {
                        Some(crate::ingest::source_declarations::ConstantEffect::All) => {
                            facts.incomplete = true
                        }
                        Some(crate::ingest::source_declarations::ConstantEffect::Name(name)) => {
                            facts.uncertain_members.insert(name);
                        }
                        None => {}
                    }
                }
            }
            let mut previous = owner.clone();
            for ancestor in ns.ancestors() {
                match ancestor {
                    Ancestor::Complete(other) if other != id => {
                        if let Some(d) = graph.declarations().get(other) {
                            if crate::ingest::source_declarations::constant_effect(d.name())
                                .is_some()
                            {
                                continue;
                            }
                            if d.as_namespace().is_some() && d.kind() != "SingletonClass" {
                                previous = owner_id(hoisted_name(d.name()).as_ref());
                                facts.ancestors.push(Step::Namespace(previous.clone()));
                            }
                        }
                    }
                    Ancestor::Partial(name) => {
                        let name = graph
                            .names()
                            .get(name)
                            .and_then(|n| graph.strings().get(n.str()))
                            .map(|s| s.as_str())
                            .unwrap_or("unresolved ancestor");
                        use crate::ingest::source_declarations::{ConstantEffect, constant_effect};
                        let through = match constant_effect(name) {
                            Some(ConstantEffect::Name(_)) => continue,
                            Some(ConstantEffect::All) => previous.0.clone(),
                            None => Symbol::new(name),
                        };
                        facts.ancestors.push(Step::Unknown(through));
                    }
                    _ => {}
                }
            }
            namespaces.insert(owner, facts);
        }
        // Declaration names retain direct ownership, including Object's
        // top-level names. Aliases retain their own name and provenance.
        for (raw, declaration) in graph.declarations() {
            if declaration.as_namespace().is_none()
                && !matches!(declaration.kind(), "Constant" | "ConstantAlias")
            {
                continue;
            }
            if declaration.kind() == "SingletonClass" {
                continue;
            }
            let full = hoisted_name(declaration.name());
            let (owner, leaf) = full.rsplit_once("::").unwrap_or(("Object", full.as_ref()));
            if leaf.is_empty() || leaf.starts_with('<') {
                continue;
            }
            let id = graph.resolve_alias(raw).unwrap_or(*raw);
            let Some(target) = graph.declarations().get(&id) else {
                continue;
            };
            let answer = retained_answer(graph, id, target, &mut cache);
            let unverified = |declaration: &Declaration| {
                is_external_declaration(graph, declaration)
                    && !declaration.definitions().iter().any(|d| {
                        graph
                            .definitions()
                            .get(d)
                            .and_then(|d| graph.documents().get(d.uri_id()))
                            .is_some_and(|d| !d.uri().starts_with(EXTERNAL_URI_PREFIX))
                    })
            };
            let unverified_loading = unverified(declaration) || unverified(target);
            namespaces
                .entry(owner_id(owner))
                .or_default()
                .members
                .insert(
                    Symbol::new(leaf),
                    Binding {
                        answer,
                        name: ClassId(Symbol::new(full.as_ref())),
                        external: is_external_declaration(graph, declaration)
                            || is_external_declaration(graph, target),
                        declaration: *raw,
                        unverified_loading,
                    },
                );
        }
        for effect in receiver_effects(graph, declarations) {
            if let Some(owner) = effect.owner {
                let facts = namespaces.entry(owner).or_default();
                if effect.mixin_override || effect.member.is_none() {
                    facts.incomplete = true;
                } else if let Some(member) = effect.member {
                    facts.uncertain_members.insert(member);
                }
            } else {
                // An unbounded receiver can alter any namespace, including
                // one searched before the current lexical scope's ancestors.
                for facts in namespaces.values_mut() {
                    facts.incomplete = true;
                }
            }
        }
        Self { namespaces }
    }

    fn own<'a>(&'a self, owner: &ClassId, leaf: &Symbol) -> Search<'a> {
        let Some(facts) = self.namespaces.get(owner) else {
            return Search::Absent;
        };
        if let Some(binding) = facts.members.get(leaf) {
            return Search::Known(binding);
        }
        if facts.incomplete || facts.uncertain_members.contains(leaf) {
            return Search::Unknown(owner.0.clone());
        }
        Search::Absent
    }
    fn ancestors<'a>(
        &'a self,
        owner: &ClassId,
        leaf: &Symbol,
        seen: &mut HashSet<ClassId>,
    ) -> Search<'a> {
        let Some(facts) = self.namespaces.get(owner) else {
            return Search::Absent;
        };
        for step in &facts.ancestors {
            match step {
                Step::Namespace(id) if seen.insert(id.clone()) => match self.own(id, leaf) {
                    Search::Absent => {}
                    result => return result,
                },
                Step::Unknown(through) => return Search::Unknown(through.clone()),
                _ => {}
            }
        }
        Search::Absent
    }
    // A finite name-effect contract can justify a positive answer for a
    // different name. It need not turn an otherwise absent name into a
    // closed-world refusal: retain a gradual gap in that case.
    fn named_gap(&self, owner: &ClassId) -> Option<Symbol> {
        let facts = self.namespaces.get(owner)?;
        if !facts.uncertain_members.is_empty() {
            return Some(owner.0.clone());
        }
        for step in &facts.ancestors {
            if let Step::Namespace(id) = step {
                if self
                    .namespaces
                    .get(id)
                    .is_some_and(|f| !f.uncertain_members.is_empty())
                {
                    return Some(id.0.clone());
                }
            }
        }
        None
    }
    fn qualified<'a>(&'a self, owner: &ClassId, leaf: &Symbol) -> Search<'a> {
        let result = match self.own(owner, leaf) {
            Search::Absent => self.ancestors(owner, leaf, &mut HashSet::from_iter([owner.clone()])),
            result => result,
        };
        match result {
            Search::Absent => self
                .named_gap(owner)
                .map_or(Search::Absent, Search::Unknown),
            result => result,
        }
    }
    fn unqualified<'a>(&'a self, scopes: &[ClassId], leaf: &Symbol) -> Search<'a> {
        let mut seen = HashSet::default();
        for scope in scopes {
            if seen.insert(scope.clone()) {
                match self.own(scope, leaf) {
                    Search::Absent => {}
                    result => return result,
                }
            }
        }
        if let Some(current) = scopes.first() {
            match self.ancestors(current, leaf, &mut seen) {
                Search::Absent => {}
                result => return result,
            }
        }
        let object = owner_id("Object");
        let result = if seen.insert(object.clone()) {
            self.qualified(&object, leaf)
        } else {
            Search::Absent
        };
        match result {
            Search::Absent => scopes
                .iter()
                .find_map(|s| self.named_gap(s))
                .map_or(Search::Absent, Search::Unknown),
            result => result,
        }
    }
    pub fn lookup<'a>(
        &'a self,
        site: Option<&Site>,
        path: &[Symbol],
        scope: Option<&ClassId>,
    ) -> ConstantLookup<'a> {
        let written = site.map_or(path, |s| s.written.as_slice());
        let rooted = written.first().is_some_and(|p| p.as_str().is_empty());
        let mut parts = written.iter().filter(|p| !p.as_str().is_empty());
        let Some(first) = parts.next() else {
            return ConstantLookup::Absent;
        };
        if !rooted && site.is_some_and(|site| site.unresolved_scope) {
            return ConstantLookup::Unknown(Symbol::new("an unindexed lexical scope"));
        }
        let fallback;
        let scopes = match site {
            Some(s) => s.scopes.as_slice(),
            None => {
                fallback = scope.into_iter().cloned().collect::<Vec<_>>();
                fallback.as_slice()
            }
        };
        let mut result = if rooted {
            self.qualified(&owner_id("Object"), first)
        } else {
            self.unqualified(scopes, first)
        };
        let mut unverified_loading = false;
        let mut original_name: Option<ClassId> = None;
        let suffix: Vec<_> = parts.collect();
        let mut external = false;
        for (at, part) in suffix.iter().enumerate() {
            match result {
                Search::Known(binding) => {
                    unverified_loading |= binding.unverified_loading;
                    external |= binding.external;
                    let owner = original_name.as_ref().unwrap_or(&binding.name);
                    original_name = Some(ClassId(Symbol::new(&format!("{}::{}", owner.0, part))));
                    let ResolvedConstant::Namespace { class, .. } = &binding.answer else {
                        if external {
                            let mut name = original_name.clone().unwrap();
                            for rest in &suffix[at + 1..] {
                                name = ClassId(Symbol::new(&format!("{}::{}", name.0, rest)));
                            }
                            return ConstantLookup::ExternalValueNamespace {
                                original_name: name,
                                unverified_loading,
                            };
                        }
                        return ConstantLookup::Unknown(Symbol::new("a dynamic namespace value"));
                    };
                    result = self.qualified(class, part);
                }
                Search::Unknown(through) => return ConstantLookup::Unknown(through),
                Search::Absent => return ConstantLookup::Absent,
            }
        }
        match result {
            Search::Known(binding) => ConstantLookup::Known {
                binding,
                original_name: original_name.unwrap_or_else(|| binding.name.clone()),
                external: external || binding.external,
                unverified_loading: unverified_loading || binding.unverified_loading,
            },
            Search::Unknown(through) => ConstantLookup::Unknown(through),
            Search::Absent => ConstantLookup::Absent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(resolver: &ConstResolver, source: &SourceFile, needle: &str) -> (String, bool) {
        let start = source.text.find(needle).unwrap();
        let span = Span {
            file: FileId(1),
            start: start as u32,
            end: (start + needle.trim_end().len()) as u32,
        };
        match resolver
            .constant_lookup(span, &[Symbol::new("unused")], None, false)
            .unwrap()
        {
            ConstantLookup::Known {
                binding,
                unverified_loading,
                ..
            } => {
                let name = match &binding.answer {
                    ResolvedConstant::Namespace { class, .. } => class.0.as_str(),
                    ResolvedConstant::Value { name, .. } => name.0.as_str(),
                };
                (format!("known:{name}"), unverified_loading)
            }
            ConstantLookup::ExternalValueNamespace { .. } => ("dynamic".into(), true),
            ConstantLookup::Unknown(through) => (format!("unknown:{through}"), false),
            ConstantLookup::Absent => ("absent".into(), false),
        }
    }

    #[test]
    fn direct_names_survive_dynamic_effects_but_missing_names_stop_before_ancestors() {
        let source = SourceFile { path: "app.rb".into(), text: "module Root\n LEXICAL = 42\n class Later; TOKEN = 42; end\n class Child < Later\n include ::Zork::Mix\n TOKEN\n LEXICAL\n end\nend\nZork::Mix::KNOWN\nZork::Mix::MISSING\n::Root::Later::TOKEN\n".into() };
        let declarations = [SourceFile {
            path: "gem.rb".into(),
            text: format!(
                "module Zork\n module Mix\n KNOWN = nil\n include ::{}\n end\nend\n",
                crate::ingest::source_declarations::OPAQUE_CONSTANTS
            ),
        }];
        let resolver =
            ConstResolver::with_declarations(std::slice::from_ref(&source), &declarations);
        assert_eq!(
            at(&resolver, &source, "Zork::Mix::KNOWN"),
            ("known:Zork::Mix::KNOWN".into(), true)
        );
        assert_eq!(
            at(&resolver, &source, "Zork::Mix::MISSING").0,
            "unknown:Zork::Mix"
        );
        assert_eq!(at(&resolver, &source, "TOKEN\n").0, "unknown:Zork::Mix");
        assert_eq!(at(&resolver, &source, "LEXICAL\n").0, "known:Root::LEXICAL");
        assert_eq!(
            at(&resolver, &source, "::Root::Later::TOKEN"),
            ("known:Root::Later::TOKEN".into(), false)
        );
    }

    #[test]
    fn app_aliases_keep_external_target_loading_separate_from_local_models() {
        let source = SourceFile {
            path: "app.rb".into(),
            text: "class Local; end\nAlias = Zork::Client\nLocalAlias = Local\nAlias\nLocalAlias\n"
                .into(),
        };
        let declarations = [SourceFile {
            path: "gem.rb".into(),
            text: "module Zork; class Client; end; end".into(),
        }];
        let resolver =
            ConstResolver::with_declarations(std::slice::from_ref(&source), &declarations);
        assert_eq!(
            at(&resolver, &source, "Alias\n"),
            ("known:Zork::Client".into(), true)
        );
        assert_eq!(
            at(&resolver, &source, "LocalAlias\n"),
            ("known:Local".into(), false)
        );
    }

    #[test]
    fn app_definition_removes_source_loading_uncertainty_for_that_name_only() {
        let source = SourceFile {
            path: "app.rb".into(),
            text: "module Zork\n class Present; end\nend\nZork::Present\nZork::OnlyInSources\n"
                .into(),
        };
        let declarations = [SourceFile {
            path: "gem.rb".into(),
            text: "module Zork\n class Present; end\n class OnlyInSources; end\nend\n".into(),
        }];
        let resolver =
            ConstResolver::with_declarations(std::slice::from_ref(&source), &declarations);
        assert_eq!(
            at(&resolver, &source, "Zork::Present"),
            ("known:Zork::Present".into(), false)
        );
        assert_eq!(
            at(&resolver, &source, "Zork::OnlyInSources"),
            ("known:Zork::OnlyInSources".into(), true)
        );
    }
}
