//! Regions: the methods in a compiled file, each with what the analyzer
//! knows about it. Passes read them for application knowledge (types,
//! effects) about the code they rewrite; the report lists them.
//!
//! A region is one `def` written in a compiled file. Rewrites are made in
//! place, so any `def` can be edited whatever surrounds it; a region is
//! just where the analyzer's facts about one are found.

use crate::det::HashMap;

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
pub struct Analyzed<'app> {
    pub body: &'app Expr,
    pub effects: &'app EffectSet,
    /// `None` where the IR does not record it (controller actions).
    pub mutates_self: Option<bool>,
    pub signature: Option<&'app Ty>,
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
            let (mut expressions, mut typed, mut effects) = (0, 0, copy.effects.is_pure());
            let mut writes = false;
            walk(copy.body, &mut |expression| {
                expressions += 1;
                if expression.ty.as_ref().is_some_and(|ty| !matches!(ty, Ty::Var { .. } | Ty::Untyped)) {
                    typed += 1;
                }
                effects &= expression.effects.is_pure();
                writes |= match &*expression.node {
                    ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => writes_state(target),
                    ExprNode::MultiAssign { targets, .. } => targets.iter().any(writes_state),
                    ExprNode::Yield { .. } => true,
                    _ => false,
                };
            });
            knowledge.writes_state |= writes;
            knowledge.no_recorded_effects &= effects;
            if least.is_none_or(|(_, least_typed)| typed < least_typed) {
                least = Some((expressions, typed));
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

fn walk<'app>(expression: &'app Expr, visit: &mut dyn FnMut(&'app Expr)) {
    visit(expression);
    expression.node.for_each_child(&mut |child| walk(child, visit));
}

/// A method in a compiled file.
pub struct Region<'app> {
    /// The class or module whose body holds the `def`, fully qualified
    /// (`Object` for one at a file's top level). For a method defined on
    /// another object (`def OTHER.name`, or a `def` in `class << OTHER`),
    /// that object's source text, `OTHER`.
    pub owner: String,
    pub name: String,
    /// `def self.name`, `def other.name`, or a `def` inside `class << self`.
    pub singleton: bool,
    pub file: FileId,
    /// The file's path relative to the app root.
    pub path: String,
    /// The whole definition, `def` through `end`.
    pub definition_span: Span,
    /// Line of `def`, 1-based.
    pub line: u32,
    pub analyzed: Vec<Analyzed<'app>>,
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
pub fn find<'app>(app: &'app App, selection: &Selection, file: FileId, path: &str) -> Vec<Region<'app>> {
    let analyzed = analyzed_by_name(app);
    let Some(source) = (file.0 as usize).checked_sub(1).and_then(|index| app.sources.get(index)) else { return Vec::new() };
    let parsed = ruby_prism::parse(source.text.as_bytes());
    let mut finder = Finder { file, selection, singletons: Vec::new(), found: Vec::new() };
    finder.visit(&parsed.node());
    let mut regions: Vec<Region<'app>> = finder
        .found
        .into_iter()
        .map(|found| {
            let (line, _) = source.line_col(found.definition_span.start);
            let copies: Vec<Analyzed<'app>> =
                analyzed.get(&found.name_span).map(|methods| methods.iter().map(|method| method.view()).collect()).unwrap_or_default();
            let knowledge = Knowledge::of(&copies);
            Region {
                owner: found.owner,
                name: found.name,
                singleton: found.singleton,
                file,
                path: path.to_string(),
                definition_span: found.definition_span,
                line,
                analyzed: copies,
                knowledge,
            }
        })
        .collect();
    regions.sort_by_key(|region| region.line);
    regions
}

struct Found {
    owner: String,
    name: String,
    name_span: Span,
    definition_span: Span,
    singleton: bool,
}

struct Finder<'selection> {
    file: FileId,
    selection: &'selection Selection,
    /// The `class << X` bodies around the visitor: `None` for `self`, else
    /// `X`'s source text.
    singletons: Vec<Option<String>>,
    found: Vec<Found>,
}

/// `None` when `node` is `self`, else its source text.
fn other_object(node: &ruby_prism::Node<'_>) -> Option<String> {
    if node.as_self_node().is_some() {
        return None;
    }
    Some(String::from_utf8_lossy(node.location().as_slice()).into_owned())
}

impl<'parse> Visit<'parse> for Finder<'_> {
    // A `class` or `module` body opens a new definition context: a `def` in
    // it is an instance method of that class, even inside `class << X`.
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'parse>) {
        let enclosing_singletons = std::mem::take(&mut self.singletons);
        ruby_prism::visit_class_node(self, node);
        self.singletons = enclosing_singletons;
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'parse>) {
        let enclosing_singletons = std::mem::take(&mut self.singletons);
        ruby_prism::visit_module_node(self, node);
        self.singletons = enclosing_singletons;
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'parse>) {
        self.singletons.push(other_object(&node.expression()));
        ruby_prism::visit_singleton_class_node(self, node);
        self.singletons.pop();
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        let span = |location: ruby_prism::Location<'_>| Span {
            file: self.file,
            start: location.start_offset() as u32,
            end: location.end_offset() as u32,
        };
        let name_span = span(node.name_loc());
        let receiver = node.receiver();
        let other = match &receiver {
            Some(receiver) => other_object(receiver),
            None => self.singletons.last().cloned().flatten(),
        };
        let owner = other.unwrap_or_else(|| self.selection.owner_of(name_span).unwrap_or("Object").to_string());
        self.found.push(Found {
            owner,
            name: text(node.name().as_slice()),
            name_span,
            definition_span: span(node.location()),
            singleton: receiver.is_some() || !self.singletons.is_empty(),
        });
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// One IR method, as the index holds it.
enum IrMethod<'app> {
    Def(&'app crate::dialect::MethodDef),
    Action(&'app crate::dialect::Action),
}

impl<'app> IrMethod<'app> {
    fn view(&self) -> Analyzed<'app> {
        match *self {
            IrMethod::Def(method) => Analyzed {
                body: &method.body,
                effects: &method.effects,
                mutates_self: Some(method.mutates_self),
                signature: method.signature.as_ref(),
            },
            IrMethod::Action(action) => Analyzed { body: &action.body, effects: &action.effects, mutates_self: None, signature: None },
        }
    }
}

/// Every analyzed method, keyed by the span of the name in its `def`.
fn analyzed_by_name(app: &App) -> HashMap<Span, Vec<IrMethod<'_>>> {
    let mut index: HashMap<Span, Vec<IrMethod<'_>>> = HashMap::default();
    fn add<'app>(index: &mut HashMap<Span, Vec<IrMethod<'app>>>, span: Span, method: IrMethod<'app>) {
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
