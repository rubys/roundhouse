//! Finite Concern class-object configuration, not a Ruby interpreter.
//!
//! Only a keyword-rest hash writer paired with `@slot || {}` is expanded.
//! Methods inherit; stored values and fresh default allocations do not.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{
    ClassConfigurationRole, Controller, ControllerBodyItem, MethodDef, MethodReceiver,
};
use crate::expr::{BoolOpKind, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};

use super::app::{
    ControllerConcernSurface, concern_class_method_catalog, controller_concern_surfaces,
    filter_registration_order, filters_from_macro_body,
};
use super::library_class::ConcernClassMethodSpans;
use super::{IngestError, IngestResult, survey};

struct Configuration {
    carrier: ClassId,
    slot: Symbol,
    writer: MethodDef,
    reader: MethodDef,
}

pub(super) fn expand(
    app: &mut App,
    carriers: &[ConcernClassMethodSpans],
    framework_shadows: &HashSet<ClassId>,
) -> IngestResult<()> {
    let catalog = concern_class_method_catalog(&app.library_classes, carriers);
    let mut configurations: Vec<_> = catalog
        .iter()
        .flat_map(|(carrier, (methods, _))| {
            methods.iter().filter_map(|writer| {
                let slot = writer_slot(writer)?;
                let reader = methods
                    .iter()
                    .find(|m| reader_slot(m).as_ref() == Some(&slot))?;
                Some(Configuration {
                    carrier: carrier.clone(),
                    slot,
                    writer: writer.clone(),
                    reader: reader.clone(),
                })
            })
        })
        .collect();
    configurations.sort_by(|a, b| (&a.carrier, &a.writer.name).cmp(&(&b.carrier, &b.writer.name)));
    if configurations.is_empty() {
        return Ok(());
    }

    let surfaces = controller_concern_surfaces(app);
    let verified = verified_concerns(app, carriers, &surfaces.module_includes, framework_shadows);
    for controller in &mut app.controllers {
        let surface = &surfaces.controllers[&controller.name];
        let candidates: Vec<_> = configurations
            .iter()
            .filter(|c| surface.includes.contains(&c.carrier))
            .collect();
        if candidates.is_empty() {
            continue;
        }
        // Transactional per controller: on refusal the survey retains the
        // complete source body, never a partially synthesized configuration.
        if let Some(body) = survey::unwrap_or_record(expand_controller(
            controller,
            &candidates,
            surface,
            &surfaces.module_includes,
            &catalog,
            &verified,
        ))? {
            controller.body = body;
        }
    }
    Ok(())
}

/// Framework identity is evidence, not the spelling `class_methods` alone.
/// No cross-file boot-order assumptions or user-defined DSL/hook execution.
fn verified_concerns(
    app: &App,
    carriers: &[ConcernClassMethodSpans],
    module_includes: &HashMap<ClassId, Vec<ClassId>>,
    framework_shadows: &HashSet<ClassId>,
) -> HashSet<ClassId> {
    verified_framework_concerns(carriers, module_includes, framework_shadows)
        .into_iter()
        .filter(|owner| {
            // Configuration callbacks can mutate receiver state too. Only
            // the existing complete filter-DSL contract may coexist.
            !app.library_classes
                .iter()
                .filter(|lc| &lc.name == owner)
                .any(|lc| {
                    let overrides_api = lc.methods.iter().any(overrides_framework_api);
                    overrides_api
                        || lc.unknown_calls.iter().any(|expr| match &*expr.node {
                            ExprNode::Send {
                                recv: None, method, ..
                            } if method.as_str() == "extend" => false,
                            ExprNode::Send {
                                recv: None,
                                method,
                                args,
                                block: Some(block),
                                ..
                            } if method.as_str() == "included" && args.is_empty() => {
                                match &*block.node {
                                    ExprNode::Lambda { body, .. } => {
                                        filters_from_macro_body(body, owner).is_none()
                                    }
                                    _ => true,
                                }
                            }
                            _ => true,
                        })
                })
        })
        .collect()
}

/// Source singleton definitions that replace the Concern protocol.
pub(super) fn overrides_framework_api(method: &MethodDef) -> bool {
    method.receiver == MethodReceiver::Class
        && matches!(
            method.name.as_str(),
            "class_methods" | "append_features" | "included" | "extended" | "prepend_features"
        )
}

/// Shared identity/installation proof. Consumers separately admit API
/// overrides and body effects (accessors allow proven inert include hooks).
pub(super) fn verified_framework_concerns(
    carriers: &[ConcernClassMethodSpans],
    module_includes: &HashMap<ClassId, Vec<ClassId>>,
    framework_shadows: &HashSet<ClassId>,
) -> HashSet<ClassId> {
    let mut extensions: HashMap<ClassId, Vec<crate::span::Span>> = HashMap::new();
    for carrier in carriers {
        extensions
            .entry(carrier.owner.clone())
            .or_default()
            .extend(&carrier.concern_extensions);
    }
    extensions
        .iter()
        .filter_map(|(owner, spans)| {
            // A literal path still resolves through Ruby's lexical
            // constants and the innermost module's included ancestors.
            // Refuse identity barriers, without evaluating their values
            // or searching enclosing classes' superclass chains.
            let included = filter_registration_order(&[vec![owner.clone()]], module_includes);
            let lexical_scope = |scope: &str| {
                owner.0.as_str() == scope
                    || owner
                        .0
                        .as_str()
                        .strip_prefix(scope)
                        .is_some_and(|rest| rest.starts_with("::"))
            };
            let identity_scope = |scope: &str| {
                scope.is_empty()
                    || scope == "Object"
                    || lexical_scope(scope)
                    || included.iter().any(|id| id.0.as_str() == scope)
            };
            let shadows_framework = framework_shadows
                .iter()
                .any(|scope| identity_scope(scope.0.as_str()));
            let source_is_verified = !shadows_framework
                && !spans.is_empty()
                && carriers.iter().filter(|c| &c.owner == owner).all(|c| {
                    !c.has_other_extensions
                        && c.concern_calls.iter().all(|call| {
                            spans.iter().any(|extension| {
                                extension.file == call.file && extension.start < call.start
                            })
                        })
                });
            source_is_verified.then(|| owner.clone())
        })
        .collect()
}

fn expand_controller(
    controller: &Controller,
    configurations: &[&Configuration],
    surface: &ControllerConcernSurface,
    module_includes: &HashMap<ClassId, Vec<ClassId>>,
    catalog: &HashMap<ClassId, (Vec<MethodDef>, HashSet<Symbol>)>,
    verified: &HashSet<ClassId>,
) -> IngestResult<Vec<ControllerBodyItem>> {
    let refuse = |message: &str| IngestError::Unsupported {
        file: controller.name.0.as_str().to_string(),
        message: message.to_string(),
    };
    for module in &surface.includes {
        let dependencies = filter_registration_order(&[vec![module.clone()]], module_includes);
        if !verified.contains(module)
            && configurations
                .iter()
                .any(|c| dependencies.contains(&c.carrier))
        {
            return Err(refuse(
                "finite class configuration requires an unmodified ActiveSupport::Concern carrier and dependencies",
            ));
        }
    }
    let mut methods = Vec::new();
    let mut names = HashSet::new();
    let mut slots = HashSet::new();
    for config in configurations {
        for method in &catalog[&config.carrier].0 {
            if method.name_span != config.writer.name_span
                && method.name_span != config.reader.name_span
                && touches_configuration(&method.body, &[config])
            {
                return Err(refuse(
                    "additional carrier access to class configuration is not supported",
                ));
            }
        }
        // Different carriers using the same class ivar share real Ruby
        // storage. Independent carrier contracts cannot model that alias.
        if !slots.insert(&config.slot) {
            return Err(refuse(
                "aliased class configuration storage is not supported",
            ));
        }
        for (method, role) in [
            (&config.writer, ClassConfigurationRole::Writer),
            (&config.reader, ClassConfigurationRole::Reader),
        ] {
            // The analyzer's parameter table has no receiver-kind key.
            // Refuse rather than mixing instance and class call-site types.
            if surface.instance_methods.contains(&method.name) {
                return Err(refuse(
                    "instance/class configuration method name collision is not supported",
                ));
            }
            if !names.insert(&method.name) {
                return Err(refuse("ambiguous class configuration methods"));
            }
            if surface.inherited_includes.contains(&config.carrier) {
                continue;
            }
            let mut method = method.clone();
            method.enclosing_class = Some(controller.name.0.clone());
            if let Some(param) = method.params.first_mut() {
                // Keep **kwargs, not ingest's legacy positional-hash approximation.
                param.keyword = true;
                param.rest = true;
                param.default = None;
                param.from_kwrest = false;
            }
            methods.push(ControllerBodyItem::ClassMethod {
                method,
                configuration_slot: (config.carrier.clone(), config.slot.clone()),
                configuration_role: role,
                leading_comments: vec![],
                leading_blank_line: false,
            });
        }
    }

    let mut available = surface.inherited_includes.clone();
    let mut expanded = Vec::new();
    for item in &controller.body {
        let ControllerBodyItem::Unknown {
            expr,
            leading_comments,
            leading_blank_line,
        } = item
        else {
            expanded.push(item.clone());
            continue;
        };
        if let ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } = &*expr.node
        {
            if method.as_str() == "include" {
                for arg in args {
                    if let ExprNode::Const { path } = &*arg.node {
                        let id = ClassId(Symbol::from(
                            path.iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("::"),
                        ));
                        available.extend(filter_registration_order(&[vec![id]], module_includes));
                    }
                }
            }
            if let Some(config) = configurations.iter().find(|c| &c.writer.name == method) {
                if !available.contains(&config.carrier) {
                    return Err(refuse(
                        "class configuration call precedes its concern include",
                    ));
                }
                let mut value = match args.as_slice() {
                    [] => Expr::new(
                        expr.span,
                        ExprNode::Hash {
                            entries: vec![],
                            kwargs: false,
                        },
                    ),
                    [hash] if finite_keyword_hash(hash) => hash.clone(),
                    _ => {
                        return Err(refuse(
                            "class configuration needs literal keyword arguments",
                        ));
                    }
                };
                if let ExprNode::Hash { kwargs, .. } = &mut *value.node {
                    *kwargs = false;
                }
                expanded.push(ControllerBodyItem::ClassIvarInit {
                    expr: Expr::new(
                        expr.span,
                        ExprNode::Assign {
                            target: LValue::Ivar {
                                name: config.slot.clone(),
                            },
                            value,
                        },
                    ),
                    carrier: config.carrier.clone(),
                    leading_comments: leading_comments.clone(),
                    leading_blank_line: *leading_blank_line,
                });
                continue;
            }
        }
        if touches_configuration(expr, configurations) {
            return Err(refuse(
                "class configuration reads, blocks and nested effects at class-definition time are not supported",
            ));
        }
        expanded.push(item.clone());
    }
    expanded.extend(methods);
    Ok(expanded)
}

fn touches_configuration(expr: &Expr, configurations: &[&Configuration]) -> bool {
    let own = match &*expr.node {
        ExprNode::Send { method, .. } => configurations
            .iter()
            .any(|c| &c.writer.name == method || &c.reader.name == method),
        ExprNode::Ivar { name }
        | ExprNode::Assign {
            target: LValue::Ivar { name },
            ..
        } => configurations.iter().any(|c| &c.slot == name),
        _ => false,
    };
    let mut found = own;
    expr.node
        .for_each_child(&mut |child| found |= touches_configuration(child, configurations));
    found
}

fn single_expr(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => single_expr(&exprs[0]),
        _ => expr,
    }
}

fn writer_slot(method: &MethodDef) -> Option<Symbol> {
    let [param] = method.params.as_slice() else {
        return None;
    };
    if !(param.from_kwrest || (param.keyword && param.rest))
        || method.block_param.is_some()
        || method.has_anonymous_block
        || method.unsupported_formals.is_some()
    {
        return None;
    }
    let ExprNode::Assign {
        target: LValue::Ivar { name },
        value,
    } = &*single_expr(&method.body).node
    else {
        return None;
    };
    matches!(&*value.node, ExprNode::Var { name, .. } if name == &param.name).then(|| name.clone())
}

fn reader_slot(method: &MethodDef) -> Option<Symbol> {
    if !method.params.is_empty()
        || method.block_param.is_some()
        || method.has_anonymous_block
        || method.unsupported_formals.is_some()
    {
        return None;
    }
    let ExprNode::BoolOp {
        op: BoolOpKind::Or,
        left,
        right,
        ..
    } = &*single_expr(&method.body).node
    else {
        return None;
    };
    let ExprNode::Ivar { name } = &*left.node else {
        return None;
    };
    matches!(&*right.node, ExprNode::Hash { entries, .. } if entries.is_empty())
        .then(|| name.clone())
}

fn finite_keyword_hash(expr: &Expr) -> bool {
    let ExprNode::Hash {
        entries,
        kwargs: true,
    } = &*expr.node
    else {
        return false;
    };
    entries.iter().all(|(key, value)| {
        matches!(
            &*key.node,
            ExprNode::Lit {
                value: Literal::Sym { .. }
            }
        ) && matches!(&*value.node, ExprNode::Lit { .. })
    })
}
