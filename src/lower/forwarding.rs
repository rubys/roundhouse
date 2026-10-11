//! Project explicit keyword producer provenance only after the source
//! destination has been classified. Native full forwarders retain `**value`;
//! ordinary calls rejoin the existing keyword lowerings without a new carrier.

use crate::App;
use crate::analyze::forwarding::{
    ConstructorContract, KeywordPolicy, keyword_calls_and_constructor_contracts, keyword_refusal,
};
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ident::ClassId;
use std::collections::{HashMap, HashSet};

pub(super) fn apply(app: &mut App) -> Vec<Diagnostic> {
    let (plans, constructors) = keyword_calls_and_constructor_contracts(app);
    let names = constructor_names(app, &constructors);
    apply_with_plans(app, &plans, &names)
}

pub(super) struct ConstructorNames {
    pub(super) splat_classes: HashSet<String>,
    pub(super) unqualified_class_ids: HashMap<String, String>,
    pub(super) ambiguous_names: HashSet<String>,
    pub(super) lexical_refinement_calls: HashSet<crate::span::Span>,
    pub(super) model_names: HashSet<String>,
    /// Classes a TEST FILE carries whose `initialize` kept keyword
    /// parameters, by full and (when unambiguous) simple name.
    /// `Server.new(**options)` there (campfire's `PushServiceTestHelper`)
    /// reaches the analyzer untyped, and Ruby 3 binds the bundle to
    /// keywords only through the splat, so it is kept. Library classes
    /// are not here: their keyword splats expand (`lower::kwsplat`).
    pub(super) keyword_initializers: HashSet<String>,
}

pub(super) fn constructor_names(
    app: &App,
    constructors: &HashMap<ClassId, ConstructorContract<'_>>,
) -> ConstructorNames {
    let (unqualified_class_ids, ambiguous_names) = unqualified_constructor_names(
        app.library_classes
            .iter()
            .map(|class| &class.name)
            .chain(app.rails_application.iter().map(|class| &class.name))
            .chain(
                app.test_modules
                    .iter()
                    .flat_map(|module| module.inner_classes.iter())
                    .map(|class| &class.name),
            )
            .chain(app.models.iter().map(|model| &model.name)),
    );
    let mut splat_classes: HashSet<String> = constructors
        .iter()
        .filter_map(|(class, contract)| match contract {
            ConstructorContract::Initialize(method)
                if !method
                    .params
                    .iter()
                    .any(|p| p.rest || p.keyword || p.forwarding)
                    && method.params.iter().any(|p| p.from_keyword) =>
            {
                Some(class.0.as_str().to_string())
            }
            ConstructorContract::CustomNew(Some(_)) | ConstructorContract::UnknownLookup => {
                Some(class.0.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    splat_classes.extend(ambiguous_names.iter().cloned());
    let keyword_initializers = keyword_initializer_names(app, &ambiguous_names);
    ConstructorNames {
        splat_classes,
        unqualified_class_ids,
        ambiguous_names,
        lexical_refinement_calls: super::helper_kwargs::lexical_refinement_calls(app),
        model_names: app
            .models
            .iter()
            .map(|model| model.name.0.as_str().to_string())
            .collect(),
        keyword_initializers,
    }
}

fn keyword_initializer_names(app: &App, ambiguous: &HashSet<String>) -> HashSet<String> {
    let mut out = HashSet::new();
    let classes = app.test_modules.iter().flat_map(|module| module.inner_classes.iter());
    for class in classes {
        let keeps = class.methods.iter().any(|m| {
            m.name.as_str() == "initialize"
                && m.receiver == crate::dialect::MethodReceiver::Instance
                && m.params.iter().any(|p| p.keyword)
                && !m.params.iter().any(|p| p.forwarding)
        });
        if !keeps {
            continue;
        }
        let full = class.name.0.as_str();
        out.insert(full.to_string());
        let simple = full.rsplit("::").next().unwrap_or(full);
        if !ambiguous.contains(simple) {
            out.insert(simple.to_string());
        }
    }
    out
}

fn unqualified_constructor_names<'a>(
    class_ids: impl Iterator<Item = &'a ClassId>,
) -> (HashMap<String, String>, HashSet<String>) {
    let mut unqualified_class_ids = HashMap::new();
    let mut ambiguous_names = HashSet::new();
    for class_id in class_ids {
        let full_name = class_id.0.as_str();
        let simple_name = full_name.rsplit("::").next().unwrap_or(full_name);
        match unqualified_class_ids.entry(simple_name.to_string()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(full_name.to_string());
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if entry.get() != full_name {
                    ambiguous_names.insert(simple_name.to_string());
                }
            }
        }
    }
    for name in &ambiguous_names {
        unqualified_class_ids.remove(name);
    }
    (unqualified_class_ids, ambiguous_names)
}

pub(super) fn apply_with_plans(
    app: &mut App,
    plans: &HashMap<crate::span::Span, KeywordPolicy>,
    names: &ConstructorNames,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (span, policy) in plans {
        if matches!(
            policy,
            KeywordPolicy::Refuse | KeywordPolicy::RefuseKeywords
                | KeywordPolicy::RefuseOrdinarySuper
        ) {
            diagnostics.push(keyword_refusal(*span, *policy));
        }
    }
    fn project(
        e: &mut Expr,
        plans: &std::collections::HashMap<crate::span::Span, KeywordPolicy>,
        names: &ConstructorNames,
    ) {
        if plans.get(&e.span) == Some(&KeywordPolicy::Legacy) {
            let (args, constructor_splat) = match &mut *e.node {
                ExprNode::Send {
                    recv, method, args, ..
                } => {
                    let constructor_splat = method.as_str() == "new"
                        && args
                            .iter()
                            .any(|arg| matches!(&*arg.node, ExprNode::KeywordSplat { .. }))
                        && recv.as_ref().is_some_and(|recv| {
                            matches!(&*recv.node, ExprNode::Const { .. })
                                && (matches!(
                                    &e.diagnostic,
                                    Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
                                        if construct.as_str() == crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS
                                ) || matches!(
                                    &recv.ty,
                                    Some(crate::ty::Ty::Class { id, .. })
                                        if names.splat_classes.contains(
                                            names.unqualified_class_ids
                                                .get(id.0.as_str())
                                                .map_or(id.0.as_str(), String::as_str)
                                        ) || names.ambiguous_names.contains(id.0.as_str())
                                            || names.lexical_refinement_calls.contains(&e.span)
                                ) || (recv.ty.is_none()
                                    && matches!(&*recv.node, ExprNode::Const { path }
                                        if names.keyword_initializers.contains(
                                            &path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
                                        ))))
                        });
                    (Some(args), constructor_splat)
                }
                ExprNode::Super { args: Some(args) } => (Some(args), false),
                _ => (None, false),
            };
            if let Some(args) = args.filter(|_| !constructor_splat) {
                for arg in args {
                    if let ExprNode::KeywordSplat { value } = &mut *arg.node {
                        *arg = std::mem::replace(value, super::typing::nil_lit());
                    }
                }
            }
        }
        e.node.for_each_child_mut(&mut |c| project(c, plans, names));
    }
    super::for_each_forwarding_body(app, &mut |e| project(e, plans, names));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::unqualified_constructor_names;
    use crate::ident::{ClassId, Symbol};

    #[test]
    fn repeated_fragments_are_not_short_name_collisions() {
        let classes = [
            ClassId(Symbol::from("Outer::Item")),
            ClassId(Symbol::from("Outer::Item")),
        ];
        let (names, ambiguous) = unqualified_constructor_names(classes.iter());
        assert_eq!(names.get("Item").map(String::as_str), Some("Outer::Item"));
        assert!(ambiguous.is_empty());
    }

    #[test]
    fn distinct_classes_with_the_same_short_name_are_ambiguous() {
        let classes = [
            ClassId(Symbol::from("Outer::Item")),
            ClassId(Symbol::from("Admin::Item")),
        ];
        let (names, ambiguous) = unqualified_constructor_names(classes.iter());
        assert!(!names.contains_key("Item"));
        assert!(ambiguous.contains("Item"));
    }
}
