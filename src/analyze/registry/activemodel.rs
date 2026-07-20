//! `ActiveModel::Validations` / `Model` module surfaces and the
//! `ActiveModel::Errors` collection + individual `ActiveModel::Error`
//! classes. Extracted verbatim from `Analyzer::with_adapter`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(classes: &mut HashMap<ClassId, ClassInfo>) {
    // `ActiveModel::Validations` / `ActiveModel::Model` — mixed into
    // plain-Ruby form/query objects (`class Search; include
    // ActiveModel::Validations`). A class that includes them gains the
    // validation surface, resolved via the includer's `includes` and
    // `lookup_in_module`. Registered as module ClassInfos carrying that
    // surface. `ActiveModel::Model` bundles Validations + Conversion +
    // attribute assignment, so it gets the same predicates plus the
    // persistence-shape readers.
    {
        let errors_ty = Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Errors")),
            args: vec![],
        };
        let mut validations = ClassInfo::default();
        for (m, ty) in [
            ("valid?", Ty::Bool),
            ("invalid?", Ty::Bool),
            ("validate", Ty::Bool),
            ("validate!", Ty::Bool),
            ("errors", errors_ty.clone()),
        ] {
            validations.instance_methods.insert(Symbol::from(m), ty);
        }
        classes
            .entry(ClassId(Symbol::from("ActiveModel::Validations")))
            .or_insert(validations.clone());

        let mut model = validations;
        for (m, ty) in [
            ("persisted?", Ty::Bool),
            ("new_record?", Ty::Bool),
            ("to_model", Ty::Untyped),
            ("model_name", Ty::Untyped),
        ] {
            model.instance_methods.insert(Symbol::from(m), ty);
        }
        classes
            .entry(ClassId(Symbol::from("ActiveModel::Model")))
            .or_insert(model);
    }

    // ActiveModel::Errors — the collection returned by `model.errors`.
    // Supports count/[]/any?/each and flows a Error instance to blocks.
    let error_ty = Ty::Class {
        id: ClassId(Symbol::from("ActiveModel::Error")),
        args: vec![],
    };
    let mut errors_cls = ClassInfo::default();
    errors_cls
        .instance_methods
        .insert(Symbol::from("count"), Ty::Int);
    errors_cls
        .instance_methods
        .insert(Symbol::from("size"), Ty::Int);
    errors_cls
        .instance_methods
        .insert(Symbol::from("any?"), Ty::Bool);
    errors_cls
        .instance_methods
        .insert(Symbol::from("none?"), Ty::Bool);
    errors_cls
        .instance_methods
        .insert(Symbol::from("empty?"), Ty::Bool);
    errors_cls
        .instance_methods
        .insert(Symbol::from("include?"), Ty::Bool);
    errors_cls.instance_methods.insert(
        Symbol::from("full_messages"),
        Ty::Array { elem: Box::new(Ty::Str) },
    );
    // `errors[:title]` returns an Array<String> of messages for that attribute.
    errors_cls.instance_methods.insert(
        Symbol::from("[]"),
        Ty::Array { elem: Box::new(Ty::Str) },
    );
    errors_cls.instance_methods.insert(
        Symbol::from("messages_for"),
        Ty::Array { elem: Box::new(Ty::Str) },
    );
    // `.each` yields an Error — registered via block_params_for below.
    errors_cls
        .instance_methods
        .insert(Symbol::from("each"), error_ty.clone());
    // `errors << "message"` is the transpiled-shape idiom for adding
    // errors from a model's `validate` method. Returns the errors
    // collection (same as Array#<<). `add` is the semantically-
    // equivalent Rails idiom.
    errors_cls.instance_methods.insert(
        Symbol::from("<<"),
        Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Errors")),
            args: vec![],
        },
    );
    errors_cls.instance_methods.insert(
        Symbol::from("add"),
        Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Errors")),
            args: vec![],
        },
    );
    errors_cls.instance_methods.insert(
        Symbol::from("clear"),
        Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Errors")),
            args: vec![],
        },
    );
    classes.insert(
        ClassId(Symbol::from("ActiveModel::Errors")),
        errors_cls,
    );

    // Individual Error with its Rails API.
    let mut error_cls = ClassInfo::default();
    error_cls
        .instance_methods
        .insert(Symbol::from("full_message"), Ty::Str);
    error_cls
        .instance_methods
        .insert(Symbol::from("message"), Ty::Str);
    error_cls
        .instance_methods
        .insert(Symbol::from("attribute"), Ty::Sym);
    error_cls
        .instance_methods
        .insert(Symbol::from("type"), Ty::Sym);
    classes.insert(
        ClassId(Symbol::from("ActiveModel::Error")),
        error_cls,
    );
}
