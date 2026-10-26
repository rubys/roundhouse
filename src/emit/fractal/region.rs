//! Regions: the methods in a compiled file, each with what the analyzer
//! knows about it. Passes read them for application knowledge (types,
//! effects) about the code they rewrite; the report lists them.
//!
//! A region is one `def` written in a compiled file. Rewrites are made in
//! place, so any `def` can be edited whatever surrounds it; a region is
//! just where the analyzer's facts about one are found.

use std::collections::HashMap;

use ruby_prism::Visit;
use serde::Serialize;

use crate::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::span::{FileId, Span};
use crate::ty::Ty;

use super::select::Selection;

use crate::ingest::prism_api::PrismLocation;
/// One copy of a method in the analyzed IR. Ingest copies a concern's
/// methods into every includer, so a method written once can have
/// several, each typed against a different `self`.
pub struct Analyzed<'a> {
    pub body: &'a Expr,
    pub effects: &'a EffectSet,
    /// `None` where the IR does not record it (controller actions).
    pub mutates_self: Option<bool>,
    pub signature: Option<&'a Ty>,
}

/// A summary of the analyzer's facts about a region, for the report.
/// Every pass reads [`Region::analyzed`]
/// itself; this is not a substitute for it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Knowledge {
    /// How many IR copies the analyzer produced (0: ingest did not model
    /// the method, so a pass has only its source text).
    pub copies: usize,
    /// Expressions in the body, and how many of them have a resolved
    /// type (not a type variable, not `untyped`), counted over the copy
    /// with the fewest.
    pub expressions: usize,
    pub typed: usize,
    /// No copy records an effect. Unresolved calls carry none, so this is
    /// only meaningful when `typed == expressions`.
    pub no_recorded_effects: bool,
    /// The body assigns an instance variable, an attribute, an index or a
    /// constant, or yields to the caller's block.
    pub writes_state: bool,
    /// The analyzer's own transitive `mutates_self`, where the IR records
    /// it.
    pub mutates_self: Option<bool>,
}

impl Knowledge {
    fn of(copies: &[Analyzed<'_>]) -> Knowledge {
        if copies.is_empty() {
            return Knowledge::default();
        }
        let mut knowledge = Knowledge { copies: copies.len(), no_recorded_effects: true, ..Knowledge::default() };
        let mut least: Option<(usize, usize)> = None;
        for copy in copies {
            let (mut exprs, mut typed, mut effects) = (0, 0, copy.effects.is_pure());
            let mut writes = false;
            walk(copy.body, &mut |e| {
                exprs += 1;
                if e.ty.as_ref().is_some_and(|t| !matches!(t, Ty::Var { .. } | Ty::Untyped)) {
                    typed += 1;
                }
                effects &= e.effects.is_pure();
                writes |= match &*e.node {
                    ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => writes_state(target),
                    ExprNode::MultiAssign { targets, .. } => targets.iter().any(writes_state),
                    ExprNode::Yield { .. } => true,
                    _ => false,
                };
            });
            knowledge.writes_state |= writes;
            knowledge.no_recorded_effects &= effects;
            if least.is_none_or(|(_, t)| typed < t) {
                least = Some((exprs, typed));
            }
            knowledge.mutates_self = match (knowledge.mutates_self, copy.mutates_self) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (_, None) => None,
                (prior, Some(false)) => prior.or(Some(false)),
            };
        }
        (knowledge.expressions, knowledge.typed) = least.unwrap_or_default();
        knowledge
    }
}

fn writes_state(target: &LValue) -> bool {
    !matches!(target, LValue::Var { .. })
}

fn walk<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.node.for_each_child(&mut |child| walk(child, f));
}

/// A method in a compiled file.
pub struct Region<'a> {
    /// The class or module whose body holds the `def`, fully qualified
    /// (`Object` for one at a file's top level).
    pub owner: String,
    pub name: String,
    /// `def self.name`, or a `def` inside `class << self`.
    pub singleton: bool,
    pub file: FileId,
    /// The file's path relative to the app root.
    pub path: String,
    /// The whole definition, `def` through `end`.
    pub def_span: Span,
    /// Line of `def`, 1-based.
    pub line: u32,
    pub analyzed: Vec<Analyzed<'a>>,
    pub knowledge: Knowledge,
}

impl Region<'_> {
    pub fn key(&self) -> String {
        format!("{}{}{}@{}:{}", self.owner, if self.singleton { "." } else { "#" }, self.name, self.path, self.line)
    }
}

/// Every `def` written in `file` (a `def` nested in another `def` belongs
/// to the outer one's region). `path` is the file's
/// path relative to the app root.
pub fn find<'a>(app: &'a App, selection: &Selection, file: FileId, path: &str) -> Vec<Region<'a>> {
    let analyzed = analyzed_by_name(app);
    let Some(source) = (file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i)) else { return Vec::new() };
    let parsed = ruby_prism::parse(source.text.as_bytes());
    let mut finder = Finder { file, selection, singleton_depth: 0, found: Vec::new() };
    finder.visit(&parsed.node());
    let mut regions: Vec<Region<'a>> = finder
        .found
        .into_iter()
        .map(|found| {
            let (line, _) = source.line_col(found.def_span.start);
            let copies: Vec<Analyzed<'a>> = analyzed.get(&found.name_span).map(|v| v.iter().map(|a| a.view()).collect()).unwrap_or_default();
            let knowledge = Knowledge::of(&copies);
            Region {
                owner: found.owner,
                name: found.name,
                singleton: found.singleton,
                file,
                path: path.to_string(),
                def_span: found.def_span,
                line,
                analyzed: copies,
                knowledge,
            }
        })
        .collect();
    regions.sort_by_key(|r| r.line);
    regions
}

struct Found {
    owner: String,
    name: String,
    name_span: Span,
    def_span: Span,
    singleton: bool,
}

struct Finder<'s> {
    file: FileId,
    selection: &'s Selection,
    singleton_depth: usize,
    found: Vec<Found>,
}

impl<'pr> Visit<'pr> for Finder<'_> {
    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        self.singleton_depth += 1;
        ruby_prism::visit_singleton_class_node(self, node);
        self.singleton_depth -= 1;
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        let span = |loc: ruby_prism::Location<'_>| Span {
            file: self.file,
            start: loc.start_offset() as u32,
            end: loc.end_offset() as u32,
        };
        let name_span = span(node.name_loc());
        let owner = self.selection.owner_of(name_span).unwrap_or("Object");
        self.found.push(Found {
            owner: owner.to_string(),
            name: text(node.name().as_slice()),
            name_span,
            def_span: span(node.location()),
            singleton: node.receiver().is_some() || self.singleton_depth > 0,
        });
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// One IR method, as the index holds it.
enum IrMethod<'a> {
    Def(&'a crate::dialect::MethodDef),
    Action(&'a crate::dialect::Action),
}

impl<'a> IrMethod<'a> {
    fn view(&self) -> Analyzed<'a> {
        match *self {
            IrMethod::Def(m) => Analyzed {
                body: &m.body,
                effects: &m.effects,
                mutates_self: Some(m.mutates_self),
                signature: m.signature.as_ref(),
            },
            IrMethod::Action(a) => Analyzed { body: &a.body, effects: &a.effects, mutates_self: None, signature: None },
        }
    }
}

/// Every analyzed method, keyed by the span of the name in its `def`.
fn analyzed_by_name(app: &App) -> HashMap<Span, Vec<IrMethod<'_>>> {
    let mut index: HashMap<Span, Vec<IrMethod<'_>>> = HashMap::new();
    fn add<'a>(index: &mut HashMap<Span, Vec<IrMethod<'a>>>, span: Span, method: IrMethod<'a>) {
        if !span.is_synthetic() {
            index.entry(span).or_default().push(method);
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                add(&mut index, method.name_span, IrMethod::Def(method));
            }
        }
    }
    for controller in &app.controllers {
        for item in &controller.body {
            match item {
                ControllerBodyItem::Action { action, .. } => add(&mut index, action.name_span, IrMethod::Action(action)),
                ControllerBodyItem::ClassMethod { method, .. } => add(&mut index, method.name_span, IrMethod::Def(method)),
                _ => {}
            }
        }
    }
    for class in &app.library_classes {
        for method in &class.methods {
            add(&mut index, method.name_span, IrMethod::Def(method));
        }
    }
    index
}
