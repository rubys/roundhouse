//! Lower `app.seeds` (the typed `db/seeds.rb` Expr) into a `Seeds`
//! LibraryClass with a single class method `run()`. The body is the
//! seeds Expr verbatim — analyze has already attached types and
//! effects, so the walker emits `Article.create!(...)`, etc., the
//! same way it would in a controller body.
//!
//! Self-describing IR: the seeds body's typing was set during
//! analyze (DbWrite on `create!`, etc.). The lowerer just wraps it
//! in a MethodDef shell — no per-target string-shape decisions.

use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol};
use crate::lower::typing::fn_sig;
use crate::ty::Ty;

/// Build a `Seeds` LibraryClass from `app.seeds`. Returns `None`
/// when the app has no seeds file.
pub fn lower_seeds_to_library_class(app: &App) -> Option<LibraryClass> {
    let body = app.seeds.as_ref()?.clone();
    let owner = ClassId(Symbol::from("Seeds"));
    let method = MethodDef {
        name: Symbol::from("run"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    };
    Some(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
    })
}
