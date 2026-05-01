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
use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::ident::Symbol;
use crate::lower::typing::fn_sig;
use crate::ty::Ty;

/// Build the `Seeds` module as a single LibraryFunction:
/// `Seeds.run() -> nil` carrying the typed seeds Expr from
/// `app.seeds` verbatim. Empty when the app has no seeds file.
pub fn lower_seeds_to_library_functions(app: &App) -> Vec<LibraryFunction> {
    let Some(body) = app.seeds.as_ref().cloned() else {
        return Vec::new();
    };
    let module_path = vec![Symbol::from("Seeds")];
    vec![LibraryFunction {
        module_path,
        name: Symbol::from("run"),
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
    }]
}
