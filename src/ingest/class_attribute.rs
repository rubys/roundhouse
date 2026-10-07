//! A `class_attribute` a Concern declares in `included do`, written by
//! the Concern's class methods from the includer's class body.
//!
//! ```ruby
//! included { class_attribute :_preload_definitions, default: [] }
//! class_methods do
//!   def preload_site_configs(codes, **options) = add_preload_definition(...)
//!   private def add_preload_definition(...) = self._preload_definitions += [...]
//! end
//! ```
//!
//! Rails runs a class-body macro when the class loads, with whatever
//! arguments it is given, and those may be computed then. So the macro is
//! not evaluated here: the emitted class makes the same call when it
//! loads. Each direct includer gets the Concern's class methods as its
//! own, the attribute as a class instance variable seeded with its
//! default, and a reader. A subclass that never writes the attribute
//! reads its parent's, which is `class_attribute` inheritance; a write
//! replaces the value on that class only.
//!
//! All or nothing per carrier: an `included` statement other than
//! `class_attribute` or filter DSL, or a class method that touches the
//! attribute any other way, leaves the carrier as it was, ledgered.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{ClassConfigurationRole, ControllerBodyItem, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};

use super::app::{concern_class_method_catalog, controller_concern_surfaces, filters_from_macro_body};
use super::class_configuration::verified_framework_concerns;
use super::library_class::ConcernClassMethodSpans;
use super::{IngestError, survey};

struct Carrier {
    /// `class_attribute` name and its default (nil when none is given).
    attributes: Vec<(Symbol, Expr)>,
    /// Class methods with attribute writes rewritten to the class ivar,
    /// each with the attribute it writes (the first when it writes none).
    methods: Vec<(MethodDef, Symbol)>,
}

pub(super) fn expand(
    app: &mut App,
    carriers: &[ConcernClassMethodSpans],
    framework_shadows: &HashSet<ClassId>,
) {
    let catalog = concern_class_method_catalog(&app.library_classes, carriers);
    let surfaces = controller_concern_surfaces(app);
    let verified = verified_framework_concerns(carriers, &surfaces.module_includes, framework_shadows);

    let mut admitted: HashMap<ClassId, Carrier> = HashMap::new();
    for lc in &app.library_classes {
        if !verified.contains(&lc.name) {
            continue;
        }
        let Some(attributes) = included_attributes(&lc.unknown_calls, &lc.name) else {
            // Only ledger when `included` actually declares a
            // `class_attribute` and also runs something else. A verified
            // Concern whose `included` is only `attr_accessor` / custom
            // hooks is not a class_attribute carrier — surveying it
            // polluted store-check and model-concern suites.
            if included_declares_class_attribute(&lc.unknown_calls) {
                survey::record(&IngestError::Unsupported {
                    file: lc.name.0.as_str().to_string(),
                    message: "class_attribute carrier `included do` must contain only class_attribute and filter DSL"
                        .to_string(),
                });
            }
            continue;
        };
        if attributes.is_empty() {
            continue;
        }
        let names: Vec<Symbol> = attributes.iter().map(|(n, _)| n.clone()).collect();
        let methods = catalog.get(&lc.name).map(|(m, _)| m.clone()).unwrap_or_default();
        // One slot per method: one writing two attributes has no single
        // configuration slot, and the carrier is refused.
        let rewritten: Option<Vec<(MethodDef, Symbol)>> = methods
            .into_iter()
            .map(|mut m| {
                if !rewrite_writes(&mut m.body, &names) {
                    return None;
                }
                let mut written = Vec::new();
                written_slots(&m.body, &names, &mut written);
                match written.len() {
                    0 => Some((m, names[0].clone())),
                    1 => Some((m, written.remove(0))),
                    _ => None,
                }
            })
            .collect();
        match rewritten {
            Some(methods) => {
                admitted.insert(lc.name.clone(), Carrier { attributes, methods });
            }
            None => survey::record(&IngestError::Unsupported {
                file: lc.name.0.as_str().to_string(),
                message: "class_attribute written other than by `self.name = value`, or two written by one method, is not modeled"
                    .to_string(),
            }),
        }
    }
    if admitted.is_empty() {
        return;
    }
    // Only expand onto controllers that name the carrier (or inherit a
    // direct include). `includes` is transitive through other modules;
    // expanding a nested-only carrier would strip its class methods
    // without copying them onto that controller. Unused carriers stay
    // on the module, unsurveyed.
    admitted.retain(|carrier, _| {
        let any_direct = surfaces
            .controllers
            .values()
            .any(|surface| surface.direct_includes.contains(carrier));
        let nested_only = surfaces.controllers.values().any(|surface| {
            surface.includes.contains(carrier) && !surface.direct_includes.contains(carrier)
        });
        if nested_only {
            survey::record(&IngestError::Unsupported {
                file: carrier.0.as_str().to_string(),
                message: "class_attribute carrier included through another module is not modeled"
                    .to_string(),
            });
            return false;
        }
        any_direct
    });
    if admitted.is_empty() {
        return;
    }
    // On the module itself these methods would write an attribute the
    // module does not have; they exist only as each includer's own.
    for lc in &mut app.library_classes {
        if let Some(carrier) = admitted.get(&lc.name) {
            lc.methods.retain(|m| {
                m.receiver != MethodReceiver::Class
                    || !carrier.methods.iter().any(|(c, _)| c.name_span == m.name_span)
            });
        }
    }

    let parents: HashMap<ClassId, Option<ClassId>> = app
        .controllers
        .iter()
        .map(|c| (c.name.clone(), c.parent.clone()))
        .collect();
    for controller in &mut app.controllers {
        let surface = &surfaces.controllers[&controller.name];
        let own: Vec<&ClassId> = surface
            .direct_includes
            .iter()
            .filter(|m| admitted.contains_key(*m) && !surface.inherited_includes.contains(m))
            .collect();
        let inherited: Vec<&ClassId> = surface
            .inherited_includes
            .iter()
            .filter(|m| admitted.contains_key(*m))
            .collect();
        if own.is_empty() && inherited.is_empty() {
            continue;
        }
        let callable: HashSet<&Symbol> = own
            .iter()
            .chain(&inherited)
            .flat_map(|m| admitted[*m].methods.iter().map(|(d, _)| &d.name))
            .collect();

        let mut body = Vec::new();
        // Rails Concern inclusion is once per class; a second textual
        // `include` must not re-seed the default after macros have run.
        let mut seeded: HashSet<ClassId> = HashSet::new();
        for item in std::mem::take(&mut controller.body) {
            match &item {
                ControllerBodyItem::Unknown { expr, leading_comments, leading_blank_line } => {
                    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
                        body.push(item);
                        continue;
                    };
                    if method.as_str() == "include" {
                        body.push(item.clone());
                        // The default is set where Rails sets it: when the
                        // Concern's `included` hook runs.
                        for module in args.iter().filter_map(const_path_to_class_id) {
                            if let Some(carrier) = own.iter().find(|m| ***m == module) {
                                if !seeded.insert((*carrier).clone()) {
                                    continue;
                                }
                                for (name, default) in &admitted[*carrier].attributes {
                                    body.push(ControllerBodyItem::ClassIvarInit {
                                        expr: Expr::new(
                                            expr.span,
                                            ExprNode::Assign {
                                                target: LValue::Ivar { name: name.clone() },
                                                value: default.clone(),
                                            },
                                        ),
                                        carrier: (*carrier).clone(),
                                        leading_comments: vec![],
                                        leading_blank_line: false,
                                    });
                                }
                            }
                        }
                        continue;
                    }
                    if callable.contains(method) {
                        // The macro call itself, run when the class loads.
                        let carrier = own
                            .iter()
                            .chain(&inherited)
                            .find(|m| admitted[**m].methods.iter().any(|(d, _)| &d.name == method))
                            .expect("callable");
                        body.push(ControllerBodyItem::ClassIvarInit {
                            expr: expr.clone(),
                            carrier: (*carrier).clone(),
                            leading_comments: leading_comments.clone(),
                            leading_blank_line: *leading_blank_line,
                        });
                        continue;
                    }
                    body.push(item);
                }
                _ => body.push(item),
            }
        }

        for carrier in &own {
            let c = &admitted[*carrier];
            for (name, _) in &c.attributes {
                body.push(class_method(
                    reader(name, None, controller.name.0.as_str()),
                    carrier,
                    name,
                ));
            }
            for (method, slot) in &c.methods {
                let mut method = method.clone();
                method.enclosing_class = Some(controller.name.0.clone());
                for p in &mut method.params {
                    // Keep the keyword the source declared; ingest's
                    // positional flattening is for call sites it repairs.
                    if p.from_keyword {
                        p.keyword = true;
                        p.from_keyword = false;
                    }
                    if p.from_kwrest {
                        p.keyword = true;
                        p.rest = true;
                        p.default = None;
                        p.from_kwrest = false;
                    }
                }
                body.push(class_method(method, carrier, slot));
            }
        }
        // Unset on a subclass reads the parent's value; a write on the
        // subclass shadows it. The parent is the class the source names.
        if let Some(Some(parent)) = parents.get(&controller.name) {
            for carrier in &inherited {
                for (name, _) in &admitted[*carrier].attributes {
                    body.push(class_method(
                        reader(name, Some(parent), controller.name.0.as_str()),
                        carrier,
                        name,
                    ));
                }
            }
        }
        controller.body = body;
    }
}

fn class_method(method: MethodDef, carrier: &ClassId, slot: &Symbol) -> ControllerBodyItem {
    ControllerBodyItem::ClassMethod {
        method,
        configuration_slot: Some((carrier.clone(), slot.clone())),
        configuration_role: Some(ClassConfigurationRole::ClassAttribute),
        leading_comments: vec![],
        leading_blank_line: false,
    }
}

/// The class ivar a write also sets, so a subclass tells an attribute it
/// never wrote from one it set to nil (Spinel has no
/// `instance_variable_defined?` on a class).
pub(crate) fn written_flag(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}__written", name.as_str()))
}

/// `def self.name; @name; end`, or on a subclass
/// `def self.name; @name__written ? @name : Parent.name; end`.
fn reader(name: &Symbol, parent: Option<&ClassId>, owner: &str) -> MethodDef {
    let span = crate::span::Span::synthetic();
    let ivar = || Expr::new(span, ExprNode::Ivar { name: name.clone() });
    let body = match parent {
        None => ivar(),
        Some(parent) => Expr::new(
            span,
            ExprNode::If {
                cond: Expr::new(span, ExprNode::Ivar { name: written_flag(name) }),
                then_branch: ivar(),
                else_branch: Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const {
                                path: parent.0.as_str().split("::").map(Symbol::from).collect(),
                            },
                        )),
                        method: name.clone(),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
            },
        ),
    };
    let mut method = super::library_class::synth_attr_reader(
        &ClassId(Symbol::from(owner)),
        name,
        MethodReceiver::Class,
    );
    method.body = body;
    method.kind = crate::dialect::AccessorKind::Method;
    method
}

/// True when some `included do` body contains a `class_attribute` call,
/// whether or not the rest of the body is a supported carrier shape.
fn included_declares_class_attribute(calls: &[Expr]) -> bool {
    for call in calls {
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*call.node else {
            continue;
        };
        if method.as_str() != "included" || !args.is_empty() {
            continue;
        }
        let ExprNode::Lambda { body, .. } = &*block.node else { continue };
        let statements: Vec<&Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![body],
        };
        if statements.iter().any(|stmt| class_attribute(stmt).is_some()) {
            return true;
        }
    }
    false
}

/// `included do … end` when every statement is `class_attribute` or
/// filter DSL: the attributes it declares. None when anything else runs.
fn included_attributes(calls: &[Expr], owner: &ClassId) -> Option<Vec<(Symbol, Expr)>> {
    let mut out = Vec::new();
    for call in calls {
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*call.node else {
            continue;
        };
        if method.as_str() != "included" || !args.is_empty() {
            continue;
        }
        let ExprNode::Lambda { body, .. } = &*block.node else { return None };
        let statements: Vec<&Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![body],
        };
        for stmt in statements {
            if let Some(attr) = class_attribute(stmt) {
                out.push(attr);
            } else if filters_from_macro_body(stmt, owner).is_none() {
                return None;
            }
        }
    }
    Some(out)
}

/// `class_attribute :name` with only the options that shape accessors;
/// `default:` is the value, nil when absent.
fn class_attribute(stmt: &Expr) -> Option<(Symbol, Expr)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*stmt.node else {
        return None;
    };
    if method.as_str() != "class_attribute" {
        return None;
    }
    let (name, options) = match args.as_slice() {
        [name] => (name, None),
        [name, options] => (name, Some(options)),
        _ => return None,
    };
    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*name.node else { return None };
    let mut default = Expr::new(stmt.span, ExprNode::Lit { value: Literal::Nil });
    if let Some(options) = options {
        let ExprNode::Hash { entries, .. } = &*options.node else { return None };
        for (key, value) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*key.node else {
                return None;
            };
            match key.as_str() {
                "default" => default = value.clone(),
                // Instance accessors are not synthesized; a call to one
                // stays unresolved rather than silently answering.
                "instance_reader" | "instance_writer" | "instance_accessor"
                | "instance_predicate" => {}
                _ => return None,
            }
        }
    }
    Some((name.clone(), default))
}

/// Rewrite `self.name = v` to `@name = v`, and `self.name op= v` to
/// `@name = name op v` (each also setting `written_flag`) — the read goes
/// through the reader, so a subclass
/// appends to the value it inherits, as in Rails. False when the body
/// reaches the storage any other way: `||=`/`&&=` on the attribute, or a
/// source `@name`, which Rails keeps apart from the attribute and the
/// rewrite would alias.
fn rewrite_writes(expr: &mut Expr, names: &[Symbol]) -> bool {
    fn touches_storage(expr: &Expr, names: &[Symbol]) -> bool {
        let own = match &*expr.node {
            ExprNode::Ivar { name }
            | ExprNode::Assign { target: LValue::Ivar { name }, .. }
            | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => names.contains(name),
            ExprNode::OpAssign { target: LValue::Attr { name, .. }, op, .. } => {
                names.contains(name) && op.binary_op().is_none()
            }
            _ => false,
        };
        let mut found = own;
        expr.node.for_each_child(&mut |c| found |= touches_storage(c, names));
        found
    }
    fn rewrite(expr: &mut Expr, names: &[Symbol]) {
        expr.node.for_each_child_mut(&mut |c| rewrite(c, names));
        let span = expr.span;
        let replacement = match &*expr.node {
            ExprNode::Assign { target: LValue::Attr { recv, name }, value }
                if matches!(&*recv.node, ExprNode::SelfRef) && names.contains(name) =>
            {
                Some((name.clone(), value.clone()))
            }
            // Ingest keeps a plain `self.name = v` as the setter call.
            ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
                if matches!(&*recv.node, ExprNode::SelfRef) && args.len() == 1 =>
            {
                method
                    .as_str()
                    .strip_suffix('=')
                    .map(Symbol::from)
                    .filter(|name| names.contains(name))
                    .map(|name| (name, args[0].clone()))
            }
            ExprNode::OpAssign { target: LValue::Attr { recv, name }, op, value }
                if matches!(&*recv.node, ExprNode::SelfRef) && names.contains(name) =>
            {
                let read = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: None,
                        method: name.clone(),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                let combined = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(read),
                        method: Symbol::from(op.binary_op().expect("checked")),
                        args: vec![value.clone()],
                        block: None,
                        parenthesized: false,
                    },
                );
                Some((name.clone(), combined))
            }
            _ => None,
        };
        // `@name = v; @name__written = true; @name`: the flag after the
        // value (computing it may read the inherited one), the value last.
        if let Some((name, value)) = replacement {
            let flag = written_flag(&name);
            *expr.node = ExprNode::Seq {
                exprs: vec![
                    Expr::new(span, ExprNode::Assign { target: LValue::Ivar { name: name.clone() }, value }),
                    Expr::new(
                        span,
                        ExprNode::Assign {
                            target: LValue::Ivar { name: flag },
                            value: Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } }),
                        },
                    ),
                    Expr::new(span, ExprNode::Ivar { name }),
                ],
            };
        }
    }
    if touches_storage(expr, names) {
        return false;
    }
    rewrite(expr, names);
    true
}

/// The attributes a rewritten body stores, in order.
fn written_slots(expr: &Expr, names: &[Symbol], out: &mut Vec<Symbol>) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*expr.node {
        if names.contains(name) && !out.contains(name) {
            out.push(name.clone());
        }
    }
    expr.node.for_each_child(&mut |c| written_slots(c, names, out));
}

/// `Foo::Bar` → `ClassId("Foo::Bar")`. Shared with the finite class-
/// configuration expander so both Concern walks join Const paths once.
pub(crate) fn const_path_to_class_id(expr: &Expr) -> Option<ClassId> {
    let ExprNode::Const { path } = &*expr.node else { return None };
    Some(ClassId(Symbol::from(
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
    )))
}
