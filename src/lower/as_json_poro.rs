//! `render json: <PORO>` — a plain object has no `as_json`, so the
//! encoder falls through to `to_s` and the response is
//! `"#<Opengraph::Metadata:0x0000000121772a50>"`.
//!
//! Rails does not have this problem because ActiveSupport puts `as_json`
//! on `Object` itself:
//!
//! ```text
//! def as_json(options = nil)
//!   if respond_to?(:to_hash) then to_hash.as_json(options)
//!   else instance_values.as_json(options)
//!   end
//! end
//! ```
//!
//! `instance_values` is reflection — a String-keyed Hash of every
//! instance variable — which the shared runtime cannot have
//! ([[feedback_runtime_must_be_statically_resolvable]]). The compiler
//! already knows the answer, so it writes it down: a class the app
//! renders as JSON gets an `as_json` built from its own attribute
//! readers.
//!
//! DEMAND-GATED, the way `to_attrs` is. Only a class actually reached by
//! `render json:` is given one — a method on every PORO in the app would
//! be dead weight, and on a strict target it is dead weight that has to
//! type-check.
//!
//! DIVERGENCE, ledgered: Rails' `instance_values` also carries the
//! framework's own ivars. Measured against ActiveModel 8.1, `render
//! json:` on campfire's `Opengraph::Metadata` yields
//! `title/url/context_for_validation/errors` — the last two are
//! validation bookkeeping no client reads, and inventing them here would
//! mean naming internals of a shim rather than the app's own surface.
//! What the app DECLARED is what ships.

use std::collections::HashSet;

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, ModelBodyItem, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

pub fn apply_as_json_synthesis(app: &mut App) {
    let wanted = json_rendered_classes(app);
    if wanted.is_empty() {
        return;
    }
    // BOTH homes. A tableless class under `app/models/` — which is what
    // campfire's `Opengraph::Metadata` is — rides in `app.models`, not
    // `app.library_classes`, so looking in one place found nothing and
    // the pass was silently inert.
    for model in &mut app.models {
        if !wanted.contains(&model.name) {
            continue;
        }
        let declares_as_json = model.body.iter().any(|item| {
            matches!(item, ModelBodyItem::Method { method, .. } if method.name.as_str() == "as_json")
        });
        if declares_as_json {
            continue;
        }
        // The `attr_*` family, splat expanded — the SAME list the
        // accessor synthesizer builds, borrowed rather than re-derived.
        // A model's own readers do not exist yet at this point in the
        // pipeline (`push_attr_accessor_methods` runs at emit time), so
        // scanning for them here finds nothing.
        let readers = super::model_to_library::markers::declared_attr_names(model);
        if readers.is_empty() {
            continue;
        }
        model.body.push(ModelBodyItem::Method {
            method: as_json_method(&model.name, &readers),
            leading_comments: Vec::new(),
            leading_blank_line: true,
        });
    }
    for lc in &mut app.library_classes {
        if !wanted.contains(&lc.name) {
            continue;
        }
        if lc.methods.iter().any(|m| m.name.as_str() == "as_json") {
            continue;
        }
        // A library class DOES already carry its readers: ingest lowers
        // `attr_reader :x` to a `MethodDef` there.
        let readers = attribute_readers(&lc.methods);
        if readers.is_empty() {
            continue;
        }
        lc.methods.push(as_json_method(&lc.name, &readers));
    }
}

/// The classes `render json: <expr>` names, by the type analyze stamped
/// on the value.
///
/// TYPE ONLY. A name fallback would be guessing at which class to grow a
/// method on, and an `as_json` on the wrong class is a method nobody
/// calls plus a payload still rendered as `to_s` — two failures for one
/// guess.
fn json_rendered_classes(app: &App) -> HashSet<ClassId> {
    let mut out = HashSet::new();
    let known: HashSet<&ClassId> = app
        .library_classes
        .iter()
        .map(|lc| &lc.name)
        .chain(app.models.iter().map(|m| &m.name))
        .collect();
    for controller in &app.controllers {
        for action in controller.actions() {
            walk(&action.body, &mut |e| {
                let ExprNode::Send { recv: None, method, args, .. } = &*e.node else { return };
                if method.as_str() != "render" {
                    return;
                }
                for arg in args {
                    let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
                            continue;
                        };
                        if value.as_str() != "json" {
                            continue;
                        }
                        if let Some(Ty::Class { id, .. }) = v.ty.as_ref() {
                            if known.contains(id) {
                                out.insert(id.clone());
                            }
                        }
                    }
                }
            });
        }
    }
    out
}

/// The class's own attribute readers, in declaration order: an instance
/// method taking nothing whose body is exactly the ivar of the same
/// name. That is the shape `attr_reader` / `attr_accessor` lowers to at
/// ingest, so this reads the app's declared surface without ingest
/// having to keep the macro around.
///
/// A computed method is deliberately NOT called — `as_json` must not
/// run app code that queries or raises just because a response is being
/// encoded.
fn attribute_readers(methods: &[MethodDef]) -> Vec<Symbol> {
    let mut out = Vec::new();
    for m in methods {
        if m.receiver != MethodReceiver::Instance || !m.params.is_empty() {
            continue;
        }
        let Some(name) = sole_ivar_read(&m.body) else { continue };
        if name == m.name && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// `def as_json(options = {}) = { "title" => @title, … }`
///
/// STRING keys, because `instance_values` is string-keyed and the
/// encoder stringifies whatever it is handed — a Symbol-keyed hash would
/// encode the same but says something the source does not.
///
/// `options` is declared and unused, which is Rails' signature: a caller
/// passing `only:`/`except:` is not modeled, and a method that took no
/// argument would raise instead of ignoring it.
fn as_json_method(owner: &ClassId, readers: &[Symbol]) -> MethodDef {
    let entries: Vec<(Expr, Expr)> = readers
        .iter()
        .map(|name| {
            (
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Str { value: name.as_str().to_string() } },
                ),
                Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() }),
            )
        })
        .collect();
    let body = Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: false });
    MethodDef {
        name: Symbol::from("as_json"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::with_default(
            Symbol::from("options"),
            Expr::new(Span::synthetic(), ExprNode::Hash { entries: vec![], kwargs: false }),
        )],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// The ivar a one-statement body reads, if that is all it does. A method
/// body arrives as a `Seq` even when the source wrote one line, so both
/// spellings have to be recognized.
fn sole_ivar_read(body: &Expr) -> Option<Symbol> {
    match &*body.node {
        ExprNode::Ivar { name } => Some(name.clone()),
        ExprNode::Seq { exprs } if exprs.len() == 1 => sole_ivar_read(&exprs[0]),
        _ => None,
    }
}

fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| walk(c, f));
}
