//! Resolve class-body definitions before checking the final emitted namespace.
//! Lowering groups actions, accessors and forwarders separately; Ruby executes
//! their declarations together, and an alias snapshots its target at that point.

use super::*;
use crate::expr::Literal;

type Key = (bool, Symbol);

fn record(
    methods: &[MethodDef],
    declared: &mut crate::det::HashSet<usize>,
    current: &mut crate::det::HashMap<Key, usize>,
    key: Key,
    index: usize,
) {
    let framework_hook = !key.0 && key.1.as_str() == "process_action";
    if let Some(previous) = current.insert(key, index) {
        let stale_helper = methods[previous].receiver == MethodReceiver::Instance
            && methods.iter().any(|method| {
                method.receiver == MethodReceiver::Class
                    && method.name == methods[previous].name
                    && method.name_span == methods[previous].name_span
            });
        let reason = if framework_hook {
            Some(
                "process_action has multiple declarations; its specialized super chain cannot be resolved",
            )
        } else if methods[previous].name != methods[index].name {
            Some(
                "the same source name lowers to different method names; declaration order cannot be preserved",
            )
        } else if stale_helper {
            Some(
                "the later definition would leave an earlier helper_method class clone; its body cannot be retained",
            )
        } else {
            None
        };
        if let Some(reason) = reason {
            crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                methods[index].name_span,
                None,
                "controller_method_collision",
                reason,
            ));
        }
    }
    declared.insert(index);
}

fn define(
    methods: &mut Vec<MethodDef>,
    declared: &mut crate::det::HashSet<usize>,
    current: &mut crate::det::HashMap<Key, usize>,
    key: Key,
    method: MethodDef,
) {
    let index = methods.len();
    methods.push(method);
    record(methods, declared, current, key, index);
}

fn symbol(expr: &Expr) -> Option<&Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value),
        _ => None,
    }
}

fn refuse_unresolved_collision(
    class: &LibraryClass,
    current: &crate::det::HashMap<Key, usize>,
    name: &str,
    span: Span,
    declaration: &str,
) {
    if current
        .keys()
        .any(|(class_side, key)| !class_side && key.as_str() == name)
        || class.methods.iter().any(|m| {
            m.receiver == MethodReceiver::Instance
                && m.name.as_str() == method_name_for_action(name)
        })
    {
        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            span,
            None,
            "controller_method_collision",
            format!(
                "{declaration} for {name} conflicts with another declaration and cannot be resolved"
            ),
        ));
    }
}

pub(super) fn resolve(
    controller: &Controller,
    class: &mut LibraryClass,
    sources: &[crate::span::SourceFile],
) {
    let mut declared = crate::det::HashSet::default();
    let mut current = crate::det::HashMap::default();
    let mut unresolved_delegates = Vec::new();
    class.unknown_calls.clear();

    for item in &controller.body {
        let source = match item {
            ControllerBodyItem::Action { action, .. } => Some((
                false,
                &action.name,
                action.name_span,
                method_name_for_action(action.name.as_str()),
            )),
            ControllerBodyItem::ClassMethod { method, .. } => {
                Some((true, &method.name, method.name_span, method.name.as_str()))
            }
            _ => None,
        };
        if let Some((class_side, name, span, lowered_name)) = source {
            // Keep the source identity as the key: two distinct Ruby names
            // that lower to one spelling (e.g. new/new_action) remain a
            // collision. Synthetic helpers are not source declarations.
            // Class methods are seeded in body order, including accessors
            // without token spans. Instance actions need source provenance
            // to distinguish them from the synthesized dispatcher.
            if (class_side || !span.is_synthetic())
                && let Some(index) = class.methods.iter().enumerate().find_map(|(i, m)| {
                    (!declared.contains(&i)
                        && (m.receiver == MethodReceiver::Class) == class_side
                        && m.name_span == span
                        && m.name.as_str() == lowered_name)
                        .then_some(i)
                })
            {
                record(
                    &class.methods,
                    &mut declared,
                    &mut current,
                    (class_side, name.clone()),
                    index,
                );
            }
            continue;
        }
        let ControllerBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } = &*expr.node
        else {
            continue;
        };
        match method.as_str() {
            "attr_reader" | "attr_writer" | "attr_accessor" => {
                for name in args.iter().filter_map(symbol) {
                    let mut generated = Vec::new();
                    if method.as_str() != "attr_writer" {
                        generated.push(crate::ingest::library_class::synth_attr_reader(
                            &controller.name,
                            name,
                            MethodReceiver::Instance,
                        ));
                    }
                    if method.as_str() != "attr_reader" {
                        generated.push(crate::ingest::library_class::synth_attr_writer(
                            &controller.name,
                            name,
                            MethodReceiver::Instance,
                        ));
                    }
                    for mut method in generated {
                        method.name_span = expr.span;
                        let key = (false, method.name.clone());
                        define(&mut class.methods, &mut declared, &mut current, key, method);
                    }
                }
            }
            "alias_method" => {
                let Some((new, old)) = args
                    .first()
                    .and_then(symbol)
                    .zip(args.get(1).and_then(symbol))
                else {
                    continue;
                };
                let Some(&index) = current.get(&(false, old.clone())) else {
                    refuse_unresolved_collision(
                        class,
                        &current,
                        new.as_str(),
                        expr.span,
                        "unresolved alias_method",
                    );
                    crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                        file: controller.name.0.to_string(),
                        message: format!(
                            "alias_method target not found: `:{old}` names no method this controller defines before the alias (inherited or concern-defined; alias not created)"
                        ),
                    });
                    continue;
                };
                if calls_super(&class.methods[index].body) {
                    refuse_unresolved_collision(
                        class,
                        &current,
                        new.as_str(),
                        expr.span,
                        "alias_method whose original super name is not retained",
                    );
                }
                let mut alias = crate::ingest::library_class::copy_aliased_method(
                    &class.methods[index],
                    new.as_str(),
                );
                alias.name_span = expr.span;
                define(
                    &mut class.methods,
                    &mut declared,
                    &mut current,
                    (false, new.clone()),
                    alias,
                );
            }
            "undef_method" => {
                for name in args.iter().filter_map(symbol) {
                    if current.remove(&(false, name.clone())).is_none() {
                        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                            file: controller.name.0.to_string(),
                            message: format!(
                                "undef_method target not removed: `:{name}` names no method this controller defines itself (inherited method stays callable; Ruby's ancestor-wide undef is not modeled)"
                            ),
                        });
                    }
                }
            }
            "delegate" => {
                // Expand separately so repeated delegated names retain each
                // declaration's provenance and aliases can snapshot them.
                class.unknown_calls.push(expr.clone());
                let forwarders =
                    crate::ingest::delegate::expand_controller_declaration(class, sources);
                if !class.unknown_calls.is_empty() {
                    for name in crate::ingest::delegate::declared_names(expr) {
                        refuse_unresolved_collision(
                            class,
                            &current,
                            name.as_str(),
                            expr.span,
                            "unsupported delegate",
                        );
                    }
                }
                unresolved_delegates.append(&mut class.unknown_calls);
                for method in forwarders {
                    let key = (false, method.name.clone());
                    define(&mut class.methods, &mut declared, &mut current, key, method);
                }
            }
            _ => {}
        }
    }
    let winners: crate::det::HashSet<_> = current.into_values().collect();
    let mut index = 0;
    class.methods.retain(|_| {
        let keep = !declared.contains(&index) || winners.contains(&index);
        index += 1;
        keep
    });
    class.unknown_calls = unresolved_delegates;
}
