//! Expand model `delegate` declarations through statically known Rails
//! associations after concern items have been spliced into their models.

use crate::dialect::{LibraryClass, MethodDef, Model, ModelBodyItem};
use crate::expr::Expr;
use crate::ident::ClassId;
use crate::ingest::delegate::{expand_delegates, is_delegate_declaration};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
struct MethodSurface {
    methods: HashMap<String, HashSet<usize>>,
}

pub(crate) fn lower_model_delegates(app: &mut crate::App) {
    let delegated_models: HashSet<_> = app
        .models
        .iter()
        .filter(|model| {
            model.body.iter().any(|item| {
                matches!(item,
                    ModelBodyItem::Unknown { expr, .. } if is_delegate_declaration(expr))
            })
        })
        .map(|model| model.name.clone())
        .collect();
    if delegated_models.is_empty() {
        return;
    }

    let body_names = crate::lower::model_to_library::schema::share_model_body_names(&app.models, &Default::default());
    let mut params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    params_specs.mark_file_fields(&app.models);
    let blocked_names: HashMap<_, _> = app
        .models
        .iter()
        .filter(|model| delegated_models.contains(&model.name))
        .map(|model| {
            let names = crate::lower::model_to_library::method_names(
                model,
                &app.models,
                &app.schema,
                &params_specs,
            );
            (model.name.clone(), names)
        })
        .collect();

    let concern_methods: HashMap<_, _> = app
        .library_classes
        .iter()
        .map(|class| {
            (
                class.name.clone(),
                (class.methods.as_slice(), class.includes.as_slice()),
            )
        })
        .collect();
    let models_by_name: HashMap<_, _> = app
        .models
        .iter()
        .map(|model| (model.name.clone(), model))
        .collect();
    let concerns_by_name: HashMap<_, _> = app
        .library_classes
        .iter()
        .map(|class| (class.name.clone(), class))
        .collect();
    let association_targets: HashSet<_> = app
        .models
        .iter()
        .filter(|model| delegated_models.contains(&model.name))
        .flat_map(|model| {
            model
                .associations()
                .flat_map(association_target_classes)
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect();
    let mut method_cache = HashMap::new();
    let available_methods: HashMap<_, _> = association_targets
        .iter()
        .filter_map(|target| {
            let model = models_by_name.get(target)?;
            Some((
                target.clone(),
                model_zero_argument_surface(
                    model,
                    &app.models,
                    &models_by_name,
                    &concerns_by_name,
                    &app.schema,
                    &params_specs,
                    &mut method_cache,
                ),
            ))
        })
        .collect();

    drop(body_names);
    for model in &mut app.models {
        let mut unknown_calls: Vec<_> = model
            .body
            .iter()
            .filter_map(|item| match item {
                ModelBodyItem::Unknown { expr, .. } if is_delegate_declaration(expr) => {
                    Some(expr.clone())
                }
                _ => None,
            })
            .collect();
        if unknown_calls.is_empty() {
            continue;
        }

        let methods: Vec<_> = model.methods().cloned().collect();
        let additional_methods = included_concern_method_bodies(model, &concern_methods);
        let supported_target_methods = supported_association_delegates(model, &available_methods);
        let blocked = blocked_names
            .get(&model.name)
            .expect("delegate model surface");
        let generated = expand_delegates(
            &model.name,
            &methods,
            &mut unknown_calls,
            &additional_methods,
            blocked,
            Some(&supported_target_methods),
            &app.sources,
        );
        let mut unexpanded_spans: Vec<_> = unknown_calls
            .iter()
            .filter(|expr| is_delegate_declaration(expr))
            .map(|expr| expr.span)
            .collect();
        model.body.retain(|item| match item {
            ModelBodyItem::Unknown { expr, .. } if is_delegate_declaration(expr) => {
                if let Some(index) = unexpanded_spans.iter().position(|span| *span == expr.span) {
                    unexpanded_spans.remove(index);
                    true
                } else {
                    false
                }
            }
            _ => true,
        });
        model
            .body
            .extend(generated.into_iter().map(|method| ModelBodyItem::Method {
                method,
                leading_comments: Vec::new(),
                leading_blank_line: true,
            }));
    }
}

fn included_concern_method_bodies(
    model: &Model,
    concern_methods: &HashMap<ClassId, (&[MethodDef], &[ClassId])>,
) -> Vec<Expr> {
    let mut pending = crate::analyze::model_includes(model);
    let mut methods = Vec::new();
    let mut seen = HashSet::new();
    while let Some(concern) = pending.pop() {
        if !seen.insert(concern.clone()) {
            continue;
        }
        if let Some((included_methods, includes)) = concern_methods.get(&concern) {
            methods.extend(
                included_methods
                    .iter()
                    .filter(|method| {
                        method.receiver == crate::dialect::MethodReceiver::Instance
                    })
                    .map(|method| method.body.clone()),
            );
            pending.extend(includes.iter().cloned());
        }
    }
    methods
}

fn exact_positional_arity(method: &MethodDef) -> Option<usize> {
    if method.unsupported_formals.is_some()
        || method.has_anonymous_block
        || method.block_param.is_some()
        || method
            .params
            .iter()
            .any(|param| param.default.is_some() || param.keyword || param.rest || param.forwarding)
    {
        return None;
    }
    Some(method.params.len())
}

fn model_zero_argument_surface(
    model: &Model,
    all_models: &[Model],
    models: &HashMap<ClassId, &Model>,
    library_classes: &HashMap<ClassId, &LibraryClass>,
    schema: &crate::schema::Schema,
    params_specs: &crate::lower::controller_to_library::params::ParamsSpecs,
    cache: &mut HashMap<ClassId, MethodSurface>,
) -> MethodSurface {
    if let Some(surface) = cache.get(&model.name) {
        return surface.clone();
    }

    let mut surface = MethodSurface::default();
    if let Some(parent) = &model.parent {
        if let Some(parent_model) = models.get(parent) {
            surface = model_zero_argument_surface(
                parent_model,
                all_models,
                models,
                library_classes,
                schema,
                params_specs,
                cache,
            );
        }
    }

    // Use the same synthesized model methods that the emitter will lower;
    // hand-building fields and association accessors here can silently drift
    // from the real model surface as Rails support grows.
    let model_methods =
        crate::lower::model_to_library::build_methods(model, all_models, schema, params_specs);
    add_method_definitions(model_methods.iter(), &mut surface);

    let mut seen = HashSet::new();
    // Surface insertion is last-definition-wins, and Ruby gives the last
    // included concern lookup precedence. Preserve source order here.
    for include in crate::analyze::model_includes(model) {
        append_concern_surface(&include, library_classes, &mut seen, &mut surface);
    }
    add_method_definitions(model.methods(), &mut surface);

    cache.insert(model.name.clone(), surface.clone());
    surface
}

fn append_concern_surface(
    concern: &ClassId,
    library_classes: &HashMap<ClassId, &LibraryClass>,
    seen: &mut HashSet<ClassId>,
    surface: &mut MethodSurface,
) {
    if !seen.insert(concern.clone()) {
        return;
    }
    let Some(class) = library_classes.get(concern) else {
        return;
    };
    for include in &class.includes {
        append_concern_surface(include, library_classes, seen, surface);
    }
    add_method_definitions(&class.methods, surface);
}

fn add_method_definitions<'a>(
    methods: impl IntoIterator<Item = &'a MethodDef>,
    surface: &mut MethodSurface,
) {
    for method in methods {
        if method.receiver != crate::dialect::MethodReceiver::Instance {
            continue;
        }
        let name = method.name.as_str().to_string();
        let arities = if method.visibility == crate::dialect::MethodVisibility::Public {
            exact_positional_arity(method)
                .filter(|_| !uses_implicit_block(&method.body))
                .map(|arity| [arity].into_iter().collect())
                .unwrap_or_default()
        } else {
            HashSet::new()
        };
        surface.methods.insert(name, arities);
    }
}

fn uses_implicit_block(expr: &Expr) -> bool {
    if matches!(&*expr.node, crate::expr::ExprNode::Yield { .. })
        || matches!(&*expr.node, crate::expr::ExprNode::Send { method, .. } if method.as_str() == "block_given?")
    {
        return true;
    }
    let mut found = false;
    expr.node
        .for_each_child(&mut |child| found |= uses_implicit_block(child));
    found
}

fn association_target_classes(association: &crate::dialect::Association) -> Vec<&ClassId> {
    use crate::dialect::Association;

    match association {
        Association::BelongsTo {
            target,
            polymorphic_targets,
            ..
        } => {
            if polymorphic_targets.is_empty() {
                vec![target]
            } else {
                polymorphic_targets.iter().collect()
            }
        }
        Association::HasOne { target, .. }
        | Association::HasMany { target, .. }
        | Association::HasAndBelongsToMany { target, .. } => vec![target],
    }
}

fn supported_association_delegates(
    model: &Model,
    available_methods: &HashMap<ClassId, MethodSurface>,
) -> HashSet<(String, String, usize)> {
    let mut supported = HashSet::new();
    for association in model.associations() {
        if !matches!(
            association,
            crate::dialect::Association::BelongsTo { .. }
                | crate::dialect::Association::HasOne { .. }
        ) {
            continue;
        }
        let targets = association_target_classes(association);
        let Some((first, rest)) = targets.split_first() else {
            continue;
        };
        let Some(first_methods) = available_methods.get(*first) else {
            continue;
        };
        for (method, arities) in &first_methods.methods {
            for arity in arities {
                if rest.iter().all(|target| {
                    available_methods.get(*target).is_some_and(|methods| {
                        methods
                            .methods
                            .get(method)
                            .is_some_and(|set| set.contains(arity))
                    })
                }) {
                    supported.insert((
                        association.name().as_str().to_string(),
                        method.clone(),
                        *arity,
                    ));
                }
            }
        }
    }
    supported
}
