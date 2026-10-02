//! Diagnostic-only ancestry evidence. Never adds types or emitted methods.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::app::App;
use crate::gems::{GemCensus, gems_owning_constant};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::generated_methods::{self, Generated};

/// Ambiguity is not absence: a legacy method-name match must not bypass it.
pub(super) enum GemClaim<'a> {
    Absent,
    Uncertain,
    Known { gem: &'a str, constant: String },
}

struct AncestryEdge {
    target: ClassId,
    parent: bool,
    /// Source includes retain actual lexical owners; empty means rooted.
    /// Existing IR lost this provenance (`None`).
    scopes: Option<Vec<ClassId>>,
}

/// Declared classes/namespaces and recorded parents/includes. Literal source
/// includes survive omitted modules and mixins consumed by shared lowering.
pub(super) struct GemAncestry {
    edges: HashMap<ClassId, Vec<AncestryEdge>>,
    /// App-declared generated surfaces in each class or module body.
    generated: HashMap<ClassId, Vec<Generated>>,
    concern_extensions: HashMap<ClassId, Vec<AncestryEdge>>,
    /// Binding values are not evaluated. Writes only veto generated
    /// ownership guesses; they do not alter the legacy ancestry policy.
    constant_bindings: HashSet<ClassId>,
    /// Unsupported declarations/configuration may reopen these owners.
    uncertain_generated_owners: HashSet<ClassId>,
}

impl GemAncestry {
    pub(super) fn new(app: &App) -> Self {
        let mut edges: HashMap<ClassId, Vec<AncestryEdge>> = HashMap::new();
        let mut add = |id: &ClassId, parent: Option<&ClassId>, includes: Vec<ClassId>| {
            let entry = edges.entry(id.clone()).or_default();
            entry.extend(
                parent
                    .cloned()
                    .into_iter()
                    .map(|target| (target, true))
                    .chain(includes.into_iter().map(|target| (target, false)))
                    .map(|(target, parent)| AncestryEdge {
                        target,
                        parent,
                        scopes: None,
                    }),
            );
        };
        for lc in &app.library_classes {
            add(&lc.name, lc.parent.as_ref(), lc.includes.clone());
        }
        for model in &app.models {
            add(
                &model.name,
                model.parent.as_ref(),
                crate::analyze::model_includes(model),
            );
        }
        for controller in &app.controllers {
            add(
                &controller.name,
                controller.parent.as_ref(),
                crate::analyze::controller_includes(controller),
            );
        }
        let mut ancestry = Self {
            edges,
            generated: HashMap::new(),
            concern_extensions: HashMap::new(),
            constant_bindings: HashSet::new(),
            uncertain_generated_owners: HashSet::new(),
        };
        // Include-only reopens may be omitted, and modeled class mixins may
        // be consumed. Supplement literal source includes without replacing
        // lossy IR edges from other declarations/concern blocks.
        for source in app.sources.iter().filter(|s| s.path.ends_with(".rb")) {
            let parsed = ruby_prism::parse(source.text.as_bytes());
            if parsed.errors().next().is_none() {
                ancestry.record_declarations(&parsed.node(), &mut Vec::new());
            }
        }
        ancestry
    }

    /// Only direct declarations provide positive ancestry/surface evidence.
    /// Unsupported executable wrappers are inspected for generated-only vetoes.
    fn record_declarations(&mut self, node: &ruby_prism::Node<'_>, scopes: &mut Vec<ClassId>) {
        if let Some(program) = node.as_program_node() {
            self.record_declarations(&program.statements().as_node(), scopes);
        } else if let Some(statements) = node.as_statements_node() {
            for statement in statements.body().iter() {
                self.record_declarations(&statement, scopes);
            }
        } else {
            let declaration = node
                .as_class_node()
                .map(|c| (c.constant_path(), c.body()))
                .or_else(|| node.as_module_node().map(|m| (m.constant_path(), m.body())));
            let Some((path, body)) = declaration else {
                self.record_uncertain_declarations(node, scopes);
                return;
            };
            let Some(id) = self.record_declaration(&path, scopes) else {
                // Failed positive ownership does not erase rooted effects
                // in this body on an unrelated, unambiguous owner.
                self.record_uncertain_declarations(node, scopes);
                return;
            };
            scopes.push(id.clone());
            if let Some(body) = body {
                let mut found = Vec::new();
                generated_methods::harvest_body(&body, &mut found);
                if !found.is_empty() {
                    self.generated.entry(id.clone()).or_default().extend(found);
                }
                self.record_literal_mixins(&body, &id, scopes, node.as_module_node().is_some());
                self.record_declarations(&body, scopes);
            }
            scopes.pop();
        }
    }

    fn mark_uncertain_owner(&mut self, path: &str, rooted: bool, scopes: &[ClassId]) {
        self.uncertain_generated_owners
            .insert(ClassId(Symbol::from(path)));
        if !rooted {
            for scope in scopes {
                self.uncertain_generated_owners
                    .insert(ClassId(Symbol::from(format!(
                        "{}::{path}",
                        scope.0.as_str()
                    ))));
            }
        }
    }

    /// A conditional declaration or literal receiver-qualified DSL call
    /// cannot provide positive evidence, but can invalidate an apparently
    /// isolated declaration elsewhere. Do not change ancestry edges here.
    fn record_uncertain_declarations(&mut self, node: &ruby_prism::Node<'_>, scopes: &[ClassId]) {
        struct Uncertain<'a> {
            ancestry: &'a mut GemAncestry,
            scopes: Vec<ClassId>,
        }
        impl Uncertain<'_> {
            fn binding(&mut self, node: &ruby_prism::Node<'_>) {
                let name = node
                    .as_constant_write_node()
                    .map(|n| n.name())
                    .or_else(|| node.as_constant_and_write_node().map(|n| n.name()))
                    .or_else(|| node.as_constant_or_write_node().map(|n| n.name()))
                    .or_else(|| node.as_constant_operator_write_node().map(|n| n.name()))
                    .or_else(|| node.as_constant_target_node().map(|n| n.name()));
                if let Some(name) = name {
                    let name = String::from_utf8_lossy(name.as_slice());
                    let id = self
                        .scopes
                        .last()
                        .map_or_else(|| name.to_string(), |owner| format!("{}::{name}", owner.0));
                    self.ancestry
                        .constant_bindings
                        .insert(ClassId(Symbol::from(id)));
                    return;
                }
                let path = node
                    .as_constant_path_write_node()
                    .map(|n| n.target().as_node())
                    .or_else(|| {
                        node.as_constant_path_and_write_node()
                            .map(|n| n.target().as_node())
                    })
                    .or_else(|| {
                        node.as_constant_path_or_write_node()
                            .map(|n| n.target().as_node())
                    })
                    .or_else(|| {
                        node.as_constant_path_operator_write_node()
                            .map(|n| n.target().as_node())
                    });
                if let Some((path, rooted)) = path.and_then(|path| GemAncestry::literal_path(&path))
                {
                    self.ancestry
                        .constant_bindings
                        .insert(ClassId(Symbol::from(path.as_str())));
                    if !rooted {
                        for scope in &self.scopes {
                            self.ancestry
                                .constant_bindings
                                .insert(ClassId(Symbol::from(format!("{}::{path}", scope.0))));
                        }
                    }
                }
            }

            fn declaration<'pr>(
                &mut self,
                path: ruby_prism::Node<'pr>,
                body: Option<ruby_prism::Node<'pr>>,
            ) {
                let Some((path, rooted)) = GemAncestry::literal_path(&path) else {
                    return;
                };
                let owner = match (rooted, self.scopes.last()) {
                    (false, Some(scope)) => format!("{}::{path}", scope.0.as_str()),
                    _ => path.clone(),
                };
                if !rooted && path.contains("::") {
                    self.ancestry
                        .mark_uncertain_owner(&path, rooted, &self.scopes);
                } else {
                    self.ancestry.mark_uncertain_owner(&owner, true, &[]);
                }
                self.scopes.push(ClassId(Symbol::from(owner)));
                if let Some(body) = body {
                    ruby_prism::Visit::visit(self, &body);
                }
                self.scopes.pop();
            }
        }
        impl<'pr> ruby_prism::Visit<'pr> for Uncertain<'_> {
            fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
                self.binding(&node);
            }
            fn visit_leaf_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
                self.binding(&node);
            }
            fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
                self.declaration(node.constant_path(), node.body());
            }
            fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
                self.declaration(node.constant_path(), node.body());
            }
            fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
                if generated_methods::dsl_for_call(call).is_some() {
                    if let Some((path, rooted)) = call
                        .receiver()
                        .and_then(|receiver| GemAncestry::literal_path(&receiver))
                    {
                        self.ancestry
                            .mark_uncertain_owner(&path, rooted, &self.scopes);
                    }
                }
                ruby_prism::visit_call_node(self, call);
            }
            fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}
        }
        ruby_prism::Visit::visit(
            &mut Uncertain {
                ancestry: self,
                scopes: scopes.to_vec(),
            },
            node,
        );
    }

    fn record_declaration(
        &mut self,
        path: &ruby_prism::Node<'_>,
        scopes: &[ClassId],
    ) -> Option<ClassId> {
        let (path, rooted) = Self::literal_path(path)?;
        // Relative compound declarations require runtime constant lookup.
        // Possible local spellings veto namespace guesses; neither contributes
        // new positive ancestry evidence.
        if !rooted && !scopes.is_empty() && path.contains("::") {
            self.mark_uncertain_owner(&path, rooted, scopes);
            for name in [
                format!("{}::{path}", scopes.last().unwrap().0.as_str()),
                path,
            ] {
                let id = ClassId(Symbol::from(name));
                self.edges.entry(id).or_default();
            }
            return None;
        }
        let name = match (rooted, scopes.last()) {
            (false, Some(owner)) => format!("{}::{path}", owner.0.as_str()),
            _ => path,
        };
        let id = ClassId(Symbol::from(name));
        self.edges.entry(id.clone()).or_default();
        Some(id)
    }

    fn record_literal_mixins(
        &mut self,
        body: &ruby_prism::Node<'_>,
        owner: &ClassId,
        scopes: &[ClassId],
        record_extensions: bool,
    ) {
        let statements = body.as_statements_node().map(|s| s.body());
        for statement in statements.iter().flat_map(|s| s.iter()) {
            let Some(call) = statement.as_call_node() else {
                continue;
            };
            if call.receiver().is_some()
                || !matches!(call.name().as_slice(), b"include" | b"extend")
                || (call.name().as_slice() == b"extend" && !record_extensions)
                || call.block().is_some()
            {
                continue;
            }
            for argument in call.arguments().iter().flat_map(|a| a.arguments().iter()) {
                if let Some((path, rooted)) = Self::literal_path(&argument) {
                    let edge = AncestryEdge {
                        target: ClassId(Symbol::from(path)),
                        parent: false,
                        scopes: Some(if rooted {
                            vec![]
                        } else {
                            scopes.iter().rev().cloned().collect()
                        }),
                    };
                    if call.name().as_slice() == b"include" {
                        self.edges.get_mut(owner).unwrap().push(edge);
                    } else {
                        // Extension evidence is kept out of instance ancestry.
                        self.concern_extensions
                            .entry(owner.clone())
                            .or_default()
                            .push(edge);
                    }
                }
            }
        }
    }

    fn literal_path(node: &ruby_prism::Node<'_>) -> Option<(String, bool)> {
        if let Some(read) = node.as_constant_read_node() {
            return Some((
                String::from_utf8_lossy(read.name().as_slice()).into_owned(),
                false,
            ));
        }
        let path = node.as_constant_path_node()?;
        let name = path.name()?;
        let name = String::from_utf8_lossy(name.as_slice());
        match path.parent() {
            Some(parent) => {
                let (parent, rooted) = Self::literal_path(&parent)?;
                Some((format!("{parent}::{name}"), rooted))
            }
            None => Some((name.into_owned(), true)),
        }
    }

    pub(super) fn receiver_gem<'a>(&self, ty: &Ty, census: &'a GemCensus) -> GemClaim<'a> {
        match ty {
            Ty::Class { id, .. } => self.class_gem(id, census),
            Ty::Union { variants } => {
                let mut claim = GemClaim::Absent;
                let mut missing = false;
                // Match nullable dispatch: nil is not a concrete receiver arm.
                // Every other arm must identify the same single gem.
                for variant in variants.iter().filter(|ty| !matches!(ty, Ty::Nil)) {
                    match self.receiver_gem(variant, census) {
                        GemClaim::Absent => missing = true,
                        GemClaim::Uncertain => return GemClaim::Uncertain,
                        GemClaim::Known { gem, constant } => {
                            if let GemClaim::Known {
                                gem: prior,
                                constant: witness,
                            } = &claim
                            {
                                if *prior != gem {
                                    return GemClaim::Uncertain;
                                }
                                if witness <= &constant {
                                    continue;
                                }
                            }
                            claim = GemClaim::Known { gem, constant };
                        }
                    }
                }
                if missing && matches!(claim, GemClaim::Known { .. }) {
                    GemClaim::Uncertain
                } else {
                    claim
                }
            }
            _ => GemClaim::Absent,
        }
    }

    fn class_gem<'a>(&self, id: &ClassId, census: &'a GemCensus) -> GemClaim<'a> {
        let mut pending = vec![id.clone()];
        let mut seen = HashSet::new();
        let mut candidates = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(edges) = self.edges.get(&id) {
                for edge in edges {
                    let Some(target) = self.resolve_edge(&id, edge, false) else {
                        return GemClaim::Uncertain;
                    };
                    pending.push(target);
                }
            } else {
                for gem in gems_owning_constant(census, id.0.as_str()) {
                    candidates
                        .entry(gem)
                        .and_modify(|witness: &mut String| {
                            if id.0.as_str() < witness.as_str() {
                                *witness = id.0.as_str().to_string();
                            }
                        })
                        .or_insert_with(|| id.0.as_str().to_string());
                }
            }
        }
        match candidates.len() {
            0 => GemClaim::Absent,
            1 => {
                let (gem, constant) = candidates.pop_first().unwrap();
                GemClaim::Known { gem, constant }
            }
            _ => GemClaim::Uncertain,
        }
    }

    /// A single statically recognized declaration reaching a receiver.
    /// Reopens/multiple occurrences require runtime configuration order
    /// we do not model, including empty/opaque occurrences. Every
    /// non-nil union arm must independently establish the same DSL.
    pub(super) fn generating_dsl(
        &self,
        ty: &Ty,
        method: &str,
        lock: &crate::gems::Lockfile,
    ) -> Option<(&'static str, ClassId)> {
        match ty {
            Ty::Class { id, .. } => self.class_generating_dsl(id, method, lock),
            Ty::Union { variants } => {
                let mut arms = variants.iter().filter(|ty| !matches!(ty, Ty::Nil));
                let first = self.generating_dsl(arms.next()?, method, lock)?;
                arms.all(|arm| {
                    self.generating_dsl(arm, method, lock)
                        .is_some_and(|(dsl, _)| dsl == first.0)
                })
                .then_some(first)
            }
            _ => None,
        }
    }

    fn class_generating_dsl(
        &self,
        receiver: &ClassId,
        method: &str,
        lock: &crate::gems::Lockfile,
    ) -> Option<(&'static str, ClassId)> {
        let mut pending = vec![receiver.clone()];
        let mut seen = HashSet::new();
        let mut found: Option<(&'static str, ClassId)> = None;
        let mut occurrences = HashMap::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            // A possible namespace reopen can also affect nested owners.
            if self.uncertain_generated_owners.iter().any(|owner| {
                id == *owner
                    || id
                        .0
                        .as_str()
                        .strip_prefix(owner.0.as_str())
                        .is_some_and(|suffix| suffix.starts_with("::"))
            }) {
                return None;
            }
            for generated in self.generated.get(&id).into_iter().flatten() {
                *occurrences.entry(generated.dsl).or_insert(0) += 1;
                if generated.requires_concern
                    && !self
                        .concern_extensions
                        .get(&id)
                        .into_iter()
                        .flatten()
                        .any(|edge| {
                            self.resolve_edge(&id, edge, true)
                                .is_some_and(|target| target.0.as_str() == "ActiveSupport::Concern")
                        })
                {
                    continue;
                }
                if !generated.surfaces.iter().any(|surface| {
                    surface.names.contains(method)
                        && surface.provider.is_none_or(|provider| lock.has(provider))
                        && surface
                            .ancestor
                            .is_none_or(|ancestor| self.descends_from(receiver, ancestor))
                }) {
                    continue;
                }
                match &found {
                    Some((dsl, _)) if *dsl != generated.dsl => return None,
                    Some(_) => {}
                    None => found = Some((generated.dsl, id.clone())),
                }
            }
            // An unresolved edge cannot establish declaration isolation.
            for edge in self.edges.get(&id).into_iter().flatten() {
                pending.push(self.resolve_edge(&id, edge, true)?);
            }
        }
        found.filter(|(dsl, _)| occurrences.get(dsl) == Some(&1))
    }

    /// A required integration is superclass evidence, not a same-named
    /// module included by the declaration or by a different receiver.
    fn descends_from(&self, receiver: &ClassId, ancestor: &str) -> bool {
        let mut pending = vec![receiver.clone()];
        let mut seen = HashSet::new();
        let mut found = false;
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            found |= id.0.as_str() == ancestor;
            for edge in self
                .edges
                .get(&id)
                .into_iter()
                .flatten()
                .filter(|edge| edge.parent)
            {
                let Some(target) = self.resolve_edge(&id, edge, true) else {
                    return false;
                };
                pending.push(target);
            }
        }
        found
    }

    fn resolve_edge(
        &self,
        owner: &ClassId,
        edge: &AncestryEdge,
        generated: bool,
    ) -> Option<ClassId> {
        let first = edge.target.0.as_str().split("::").next()?;
        if let Some(scopes) = &edge.scopes {
            for scope in scopes {
                let namespace = format!("{}::{first}", scope.0.as_str());
                if generated && self.binding_shadows(&namespace) {
                    return None;
                }
                if self.has_namespace(&namespace) {
                    let target = ClassId(Symbol::from(format!(
                        "{}::{}",
                        scope.0.as_str(),
                        edge.target.0.as_str()
                    )));
                    return self.edges.contains_key(&target).then_some(target);
                }
            }
        } else {
            // IR lost rootedness: veto every possible local shadow, including
            // the owner's own constants, rather than guess method ownership.
            let mut scope = Some(owner.0.as_str());
            while let Some(prefix) = scope {
                if generated && self.binding_shadows(&format!("{prefix}::{first}")) {
                    return None;
                }
                if self.has_namespace(&format!("{prefix}::{first}")) {
                    return None;
                }
                scope = prefix.rsplit_once("::").map(|(prefix, _)| prefix);
            }
        }
        if generated && self.binding_shadows(edge.target.0.as_str()) {
            return None;
        }
        Some(edge.target.clone())
    }

    fn binding_shadows(&self, namespace: &str) -> bool {
        self.constant_bindings.iter().any(|id| {
            let binding = id.0.as_str();
            binding == namespace
                || namespace
                    .strip_prefix(binding)
                    .is_some_and(|suffix| suffix.starts_with("::"))
        })
    }

    fn has_namespace(&self, namespace: &str) -> bool {
        let children = format!("{namespace}::");
        self.edges
            .keys()
            .any(|id| id.0.as_str() == namespace || id.0.as_str().starts_with(&children))
    }
}
