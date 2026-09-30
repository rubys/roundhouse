//! Diagnostic-only ancestry evidence. Never adds types or emitted methods.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::app::App;
use crate::gems::{GemCensus, gems_owning_constant};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Ambiguity is not absence: a legacy method-name match must not bypass it.
pub(super) enum GemClaim<'a> {
    Absent,
    Uncertain,
    Known { gem: &'a str, constant: String },
}

struct AncestryEdge {
    target: ClassId,
    /// Source includes retain actual lexical owners; empty means rooted.
    /// Existing IR lost this provenance (`None`).
    scopes: Option<Vec<ClassId>>,
}

/// Declared classes/namespaces and recorded parents/includes. Empty and
/// include-only modules are supplemented from source, not added to typed IR.
pub(super) struct GemAncestry {
    edges: HashMap<ClassId, Vec<AncestryEdge>>,
}

impl GemAncestry {
    pub(super) fn new(app: &App) -> Self {
        let mut edges: HashMap<ClassId, Vec<AncestryEdge>> = HashMap::new();
        let mut add =
            |id: &ClassId, parent: Option<&ClassId>, includes: Vec<ClassId>| {
                let entry = edges.entry(id.clone()).or_default();
                entry.extend(parent.cloned().into_iter().chain(includes).map(|target| {
                    AncestryEdge {
                        target,
                        scopes: None,
                    }
                }));
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
        let mut ancestry = Self { edges };
        // Include-only reopens may be omitted even when an earlier declaration
        // survives. Supplement all literal module includes, without replacing
        // lossy IR edges that may come from other declarations/concern blocks.
        for source in app.sources.iter().filter(|s| s.path.ends_with(".rb")) {
            let parsed = ruby_prism::parse(source.text.as_bytes());
            if parsed.errors().next().is_none() {
                ancestry.record_declarations(&parsed.node(), &mut Vec::new());
            }
        }
        ancestry
    }

    /// Deliberately no descent into methods, blocks or control flow.
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
                return;
            };
            let Some(id) = self.record_declaration(&path, scopes) else {
                return;
            };
            scopes.push(id.clone());
            if let Some(body) = body {
                if node.as_module_node().is_some() {
                    self.record_module_includes(&body, &id, scopes);
                }
                self.record_declarations(&body, scopes);
            }
            scopes.pop();
        }
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
            self.edges
                .entry(ClassId(Symbol::from(format!(
                    "{}::{path}",
                    scopes.last().unwrap().0.as_str()
                ))))
                .or_default();
            self.edges.entry(ClassId(Symbol::from(path))).or_default();
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

    fn record_module_includes(
        &mut self,
        body: &ruby_prism::Node<'_>,
        owner: &ClassId,
        scopes: &[ClassId],
    ) {
        let statements = body.as_statements_node().map(|s| s.body());
        for statement in statements.iter().flat_map(|s| s.iter()) {
            let Some(call) = statement.as_call_node() else {
                continue;
            };
            if call.receiver().is_some()
                || call.name().as_slice() != b"include"
                || call.block().is_some()
            {
                continue;
            }
            for argument in call.arguments().iter().flat_map(|a| a.arguments().iter()) {
                if let Some((path, rooted)) = Self::literal_path(&argument) {
                    self.edges.get_mut(owner).unwrap().push(AncestryEdge {
                        target: ClassId(Symbol::from(path)),
                        scopes: Some(if rooted {
                            vec![]
                        } else {
                            scopes.iter().rev().cloned().collect()
                        }),
                    });
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
                    let Some(target) = self.resolve_edge(&id, edge) else {
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

    fn resolve_edge(&self, owner: &ClassId, edge: &AncestryEdge) -> Option<ClassId> {
        let first = edge.target.0.as_str().split("::").next()?;
        if let Some(scopes) = &edge.scopes {
            for scope in scopes {
                let namespace = format!("{}::{first}", scope.0.as_str());
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
                if self.has_namespace(&format!("{prefix}::{first}")) {
                    return None;
                }
                scope = prefix.rsplit_once("::").map(|(prefix, _)| prefix);
            }
        }
        Some(edge.target.clone())
    }

    fn has_namespace(&self, namespace: &str) -> bool {
        let children = format!("{namespace}::");
        self.edges
            .keys()
            .any(|id| id.0.as_str() == namespace || id.0.as_str().starts_with(&children))
    }
}
