//! Regions: the methods a selection covers, each with what the analyzer
//! knows about it and whether Fractal can attach a variant to it.
//!
//! A region is one `def` written directly in a selected class body. The
//! shape checks here are the ones every pass inherits: a variant is
//! installed by reopening the class at its original nesting and
//! defining the method again, so the method has to be one Ruby defines
//! exactly once, plainly, in that body.

use std::collections::HashMap;

use ruby_prism::{Node, Visit};
use serde::Serialize;

use crate::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::span::{FileId, Span};
use crate::ty::Ty;

use super::select::Selection;

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

/// A summary of the analyzer's facts about a region, for the report and
/// for the runtime's verify mode. Every pass reads [`Region::analyzed`]
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

    /// Whether the runtime may run this method's original and its variant
    /// one after the other to compare them (`FRACTAL_VERIFY=1`), as far as
    /// the analyzer can tell: every expression typed, no recorded effect,
    /// no state written. A call that mutates its receiver (`@items <<
    /// item`) is invisible to these facts, which is why a pass must vouch
    /// for replay as well ([`super::Proposal::replayable`]).
    pub fn replayable(&self) -> bool {
        self.copies > 0
            && self.typed == self.expressions
            && self.no_recorded_effects
            && !self.writes_state
            && self.mutates_self != Some(true)
    }
}

fn writes_state(target: &LValue) -> bool {
    !matches!(target, LValue::Var { .. })
}

fn walk<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.node.for_each_child(&mut |child| walk(child, f));
}

/// A method in a selected class body.
pub struct Region<'a> {
    /// The selected class or module, fully qualified.
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
    /// `Err(reason)` when Fractal cannot attach a variant to this method.
    /// Passes are not offered such a region.
    pub admission: Result<(), String>,
    /// What precedes the definition in a variant's source: the file's
    /// `frozen_string_literal` comment, then the class and module headers
    /// that reproduce its lexical nesting.
    pub(super) prelude: Vec<String>,
    pub(super) closers: usize,
}

impl Region<'_> {
    pub fn key(&self) -> String {
        format!("{}{}{}@{}:{}", self.owner, if self.singleton { "." } else { "#" }, self.name, self.path, self.line)
    }
}

/// Visibility calls a `def` may be the argument of. Each takes the
/// method's name and changes only its visibility, which the runtime
/// carries over to a variant.
const VISIBILITY: [&str; 5] = ["private", "protected", "public", "private_class_method", "public_class_method"];

/// Calls whose answer depends on which file the code was loaded from.
/// A variant is evaluated from the generated initializer.
const LOCATION_DEPENDENT: [&str; 2] = ["__dir__", "require_relative"];

/// Every `def` written directly in a selected body. `path_of` gives a
/// source's path relative to the app root, or `None` when it lies outside
/// it. `untrusted` files are the ones ingest could not read faithfully.
pub fn find<'a>(
    app: &'a App,
    selection: &Selection,
    path_of: &dyn Fn(FileId) -> Option<String>,
    untrusted: &dyn Fn(&str) -> bool,
) -> Vec<Region<'a>> {
    let analyzed = analyzed_by_name(app);
    let mut files: Vec<FileId> = selection.bodies.iter().map(|(_, span)| span.file).collect();
    files.sort_by_key(|f| f.0);
    files.dedup();
    let mut regions = Vec::new();
    for file in files {
        let Some(source) = (file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i)) else { continue };
        let path = path_of(file);
        let parsed = ruby_prism::parse(source.text.as_bytes());
        let mut magic = Vec::new();
        let mut file_refusal = None;
        for comment in parsed.magic_comments() {
            let key = String::from_utf8_lossy(comment.key()).to_ascii_lowercase().replace('-', "_");
            let value = String::from_utf8_lossy(comment.value()).trim().to_string();
            match key.as_str() {
                "frozen_string_literal" => magic.push(format!("# frozen_string_literal: {value}")),
                "encoding" | "coding" if !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("utf8") => {
                    file_refusal = Some(format!("the file is {value}, and a variant's source is UTF-8"));
                }
                _ => {}
            }
        }
        if path.is_none() {
            file_refusal = Some("the file is outside the app root".to_string());
        } else if untrusted(&source.path) {
            file_refusal = Some("ingest could not read the file faithfully (a --survey gap or a syntax error)".to_string());
        }
        let mut finder = Finder { file, selection, stack: Vec::new(), found: Vec::new() };
        finder.visit(&parsed.node());
        for found in finder.found {
            let (line, _) = source.line_col(found.def_span.start);
            let copies: Vec<Analyzed<'a>> = analyzed.get(&found.name_span).map(|v| v.iter().map(|a| a.view()).collect()).unwrap_or_default();
            // The method's own reason first: it is the more specific one.
            let admission = match (found.refusal, &file_refusal) {
                (Some(reason), _) => Err(reason),
                (None, Some(reason)) => Err(reason.clone()),
                (None, None) => Ok(()),
            };
            let mut prelude = magic.clone();
            let closers = found.headers.len();
            prelude.extend(found.headers);
            let knowledge = Knowledge::of(&copies);
            regions.push(Region {
                owner: found.owner,
                name: found.name,
                singleton: found.singleton,
                file,
                path: path.clone().unwrap_or_else(|| source.path.clone()),
                def_span: found.def_span,
                line,
                analyzed: copies,
                knowledge,
                admission,
                prelude,
                closers,
            });
        }
    }
    regions.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    regions
}

struct Found {
    owner: String,
    name: String,
    name_span: Span,
    def_span: Span,
    singleton: bool,
    headers: Vec<String>,
    refusal: Option<String>,
}

struct Finder<'s, 'pr> {
    file: FileId,
    selection: &'s Selection,
    stack: Vec<Node<'pr>>,
    found: Vec<Found>,
}

impl<'pr> Visit<'pr> for Finder<'_, 'pr> {
    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        self.stack.push(node);
    }

    fn visit_branch_node_leave(&mut self) {
        self.stack.pop();
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        let span = |loc: ruby_prism::Location<'_>| Span {
            file: self.file,
            start: loc.start_offset() as u32,
            end: loc.end_offset() as u32,
        };
        let name_span = span(node.name_loc());
        // A def inside a def is defined when its outer method runs, not
        // by the class body, so it is never a region and never descended.
        let Some(owner) = self.selection.owner_of(name_span) else { return };
        let ancestors = &self.stack[..self.stack.len().saturating_sub(1)];
        let mut headers = Vec::new();
        let mut refusal = None;
        let mut in_singleton_class = false;
        for (i, ancestor) in ancestors.iter().enumerate() {
            if ancestor.as_program_node().is_some() || ancestor.as_statements_node().is_some() {
            } else if let Some(class) = ancestor.as_class_node() {
                headers.push(format!("class {}", text(class.constant_path().location().as_slice())));
            } else if let Some(module) = ancestor.as_module_node() {
                headers.push(format!("module {}", text(module.constant_path().location().as_slice())));
            } else if let Some(sclass) = ancestor.as_singleton_class_node() {
                if sclass.expression().as_self_node().is_some() {
                    headers.push("class << self".to_string());
                    in_singleton_class = true;
                } else {
                    refusal.get_or_insert_with(|| "defined inside `class << <expression>` other than self".to_string());
                }
            } else if ancestor.as_arguments_node().is_some()
                && ancestors.get(i.wrapping_sub(1)).and_then(|n| n.as_call_node()).is_some_and(|call| visibility_call(&call))
            {
            } else if ancestor.as_call_node().is_some_and(|call| visibility_call(&call)) {
            } else if let Some(call) = ancestor.as_call_node() {
                let name = text(call.name().as_slice());
                refusal.get_or_insert_with(|| format!("the definition is an argument or block of `{name}`, which may wrap or redefine it"));
            } else {
                refusal.get_or_insert_with(|| "defined inside a block, condition or other expression, so the class body may not define it".to_string());
            }
        }
        let singleton = match node.receiver() {
            None => in_singleton_class,
            Some(receiver) if receiver.as_self_node().is_some() && !in_singleton_class => true,
            Some(_) => {
                refusal.get_or_insert_with(|| "defined on a receiver other than the class itself".to_string());
                in_singleton_class
            }
        };
        if let Some(body) = node.body() {
            let mut scan = BodyScan { refusal: None };
            scan.visit(&body);
            if let Some(reason) = scan.refusal {
                refusal.get_or_insert(reason);
            }
        }
        self.found.push(Found {
            owner: owner.to_string(),
            name: text(node.name().as_slice()),
            name_span,
            def_span: span(node.location()),
            singleton,
            headers,
            refusal,
        });
    }
}

fn visibility_call(call: &ruby_prism::CallNode<'_>) -> bool {
    call.receiver().is_none() && call.block().is_none() && VISIBILITY.contains(&text(call.name().as_slice()).as_str())
}

/// Body constructs a variant cannot reproduce from another file.
struct BodyScan {
    refusal: Option<String>,
}

impl<'pr> Visit<'pr> for BodyScan {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        let name = text(node.name().as_slice());
        if node.receiver().is_none() && LOCATION_DEPENDENT.contains(&name.as_str()) {
            self.refusal.get_or_insert_with(|| format!("the body calls `{name}`, which depends on the defining file"));
        }
        ruby_prism::visit_call_node(self, node);
    }

    fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {
        self.refusal.get_or_insert_with(|| "the body defines another method".to_string());
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
