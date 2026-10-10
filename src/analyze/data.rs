//! Class identities for literal Data.define constants on source library classes.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::LibraryClassOrigin;
use crate::expr::{Expr, ExprNode, Literal, RESOLVED_DATA_FACTORY};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::ClassInfo;
use super::body::ConstResolver;

/// Register class identities for built-in `Data.define` constants with distinct
/// literal members, retaining any lifted custom methods. Generated readers stay
/// untyped; rejected declarations are absent from the returned span-to-type map.
pub(super) fn register(
    app: &App,
    resolver: &ConstResolver,
    classes: &mut HashMap<ClassId, ClassInfo>,
) -> HashMap<Span, Ty> {
    let mut factories = HashMap::new();
    // Source overrides can replace `.define`; only the built-in factory is modeled here.
    if resolver.has_source_namespace("Data") {
        return factories;
    }
    let mut register = |owner: &ClassId, name: &Symbol, value: &Expr| {
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*value.node
        else {
            return;
        };
        let ExprNode::Const { path } = &*recv.node else {
            return;
        };
        if method.as_str() != "define" || !resolver.is_runtime_class(recv.span, path, "Data") {
            return;
        }
        let mut members = HashSet::new();
        for arg in args {
            let ExprNode::Lit {
                value: Literal::Sym { value: member },
            } = &*arg.node
            else {
                return;
            };
            if !reader_name(member.as_str()) || !members.insert(member.clone()) {
                return;
            }
        }
        let Some(id) = resolver.constant_class(value.span, name.as_str()) else {
            return;
        };
        // Rehomed constants are not emitted in their original source scope.
        let custom = app.library_classes.iter().any(|class| {
            class.name == id && matches!(class.origin,
                Some(LibraryClassOrigin::DataFactory { declaration_span }) if declaration_span == value.span)
        });
        // The main branch also ingests Struct block classes into the same
        // registry. Preserve those generated declarations when a factory
        // adds its constructor and readers.
        let reopened = app.library_classes.iter().any(|class| {
            class.name == id && !class.is_module && class.parent.is_none()
        });
        if id.0.as_str() != format!("{}::{}", owner.0.as_str(), name.as_str())
            || (classes.contains_key(&id) && !custom && !reopened)
            || (custom && app.library_classes.iter().filter(|class| class.name == id).count() != 1)
        {
            return;
        }
        let instance = Ty::Class {
            id: id.clone(),
            args: vec![].into(),
        };
        let mut info = classes.remove(&id).unwrap_or_default();
        info.class_methods
            .insert(Symbol::from("new"), instance.clone());
        info.declares_constructor = true;
        // A member declaration establishes a reader, not its value type.
        // Data has no generated writers. A block method of the same name
        // overrides the reader, as it does in Ruby.
        for member in members {
            info.instance_methods.entry(member).or_insert(Ty::Untyped);
        }
        info.instance_methods.entry(Symbol::from("with")).or_insert(instance.clone());
        classes.insert(id, info);
        factories.insert(value.span, instance);
    };
    for class in &app.library_classes {
        for (name, value) in &class.constants {
            register(&class.name, name, value);
        }
    }
    factories
}

/// Report lifted custom factories whose declaration was not admitted by analysis
/// or whose generated class is subclassed. Lifting methods alone does not establish
/// that the original factory belongs to the supported subset.
pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    if !app.library_classes.iter().any(|class| matches!(class.origin, Some(LibraryClassOrigin::DataFactory { .. }))) {
        return Vec::new();
    }
    let resolver = app.const_resolver.for_sources(&app.sources);
    let mut diagnostics = Vec::new();
    for factory in &app.library_classes {
        let Some(LibraryClassOrigin::DataFactory { declaration_span }) = factory.origin else {
            continue;
        };
        let admitted = app.library_classes.iter().flat_map(|class| &class.constants)
            .any(|(_, value)| value.span == declaration_span && value.decisions & RESOLVED_DATA_FACTORY != 0);
        let subclassed = app.library_classes.iter().any(|class| {
            class.parent.as_ref().is_some_and(|parent| {
                let path: Vec<_> = parent.0.as_str().split("::").map(Symbol::from).collect();
                resolver.declaration_name(class.parent_span, &path) == Some(factory.name.0.as_str())
            })
        });
        if !admitted || subclassed {
            diagnostics.push(Diagnostic::unsupported(declaration_span, None, "Data.define",
                "custom Data factories require the built-in Data, distinct literal member names, and no class reopening or subclassing"));
        }
    }
    diagnostics
}

fn reader_name(member: &str) -> bool {
    let bare = member
        .strip_suffix('?')
        .or_else(|| member.strip_suffix('!'))
        .unwrap_or(member);
    let mut chars = bare.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}
