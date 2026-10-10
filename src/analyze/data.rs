//! Class identities for literal Data.define constants on source library classes.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::ClassInfo;
use super::body::ConstResolver;

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
        if id.0.as_str() != format!("{}::{}", owner.0.as_str(), name.as_str()) {
            return;
        }
        // The one class that may already be registered under this name is
        // the factory's own block (`Data.define(:a) do def … end`, ingested
        // as a parentless library class of that name): the factory adds
        // `new` and the readers beside its methods. Anything else — a
        // source class that happens to share the name — is not this Data.
        let reopened = app.library_classes.iter().any(|class| {
            class.name == id && !class.is_module && class.parent.is_none()
        });
        if classes.contains_key(&id) && !reopened {
            return;
        }
        let instance = Ty::Class {
            id: id.clone(),
            args: vec![],
        };
        let mut info = classes.remove(&id).unwrap_or_default();
        info.class_methods
            .insert(Symbol::from("new"), instance.clone());
        // A member declaration establishes a reader, not its value type.
        // Data has no generated writers. A block method of the same name
        // overrides the reader, as it does in Ruby.
        for member in members {
            info.instance_methods.entry(member).or_insert(Ty::Untyped);
        }
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
