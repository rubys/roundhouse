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
//!
//! ## The encoder, monomorphized
//!
//! `as_json` answers the Hash; something still has to turn it into
//! text. The CRuby overlay's `ActionController::JsonRender.encode` walks
//! that Hash reflectively (`respond_to?(:as_json)`, `case value when
//! Hash …`), which no strict target can bind — on the spinel tree the
//! constant is unresolved and every `POST /unfurl_links` is a 500.
//!
//! The key set is known here, so the text is written down too: the same
//! readers become an `as_json_str` writer ([`super::as_json_writer`],
//! the straight-line `io << "\"title\":" << JsonBuilder.encode_value(…)`
//! shape jbuilder templates lower to), and the call site becomes the
//! Rails idiom for an already-encoded body —
//!
//! ```text
//! render json: opengraph
//! render plain: opengraph.as_json_str, content_type: "application/json"
//! ```
//!
//! — which the controller rewrite already lowers on every target, with
//! the author's `content_type:` standing. `JsonBuilder` ships in the
//! shared runtime, so the ruby lane runs the very writer the compiled
//! lane does: one oracle, one text. A `render json:` whose value is not
//! a class this pass wrote a writer for (a Hash literal, a Relation, a
//! model with its own `as_json` — see `as_json_shape`'s known gaps)
//! keeps the runtime encoder, CRuby-only as before.

use std::collections::HashSet;

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, ModelBodyItem, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::as_json_shape::{JsonPair, PairValue};
use super::as_json_writer::{writer_method, WRITER_METHOD};
use super::typing::with_ty;

pub fn apply_as_json_synthesis(app: &mut App) {
    let wanted = json_rendered_classes(app);
    if wanted.is_empty() {
        return;
    }
    // BOTH homes. A tableless class under `app/models/` — which is what
    // campfire's `Opengraph::Metadata` is — rides in `app.models`, not
    // `app.library_classes`, so looking in one place found nothing and
    // the pass was silently inert.
    // The classes given a writer, so the call sites below rewrite for
    // exactly those and no other.
    let mut written: HashSet<ClassId> = HashSet::new();
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
        model.body.push(ModelBodyItem::Method {
            method: as_json_str_method(&model.name, &readers),
            leading_comments: Vec::new(),
            leading_blank_line: true,
        });
        written.insert(model.name.clone());
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
        lc.methods.push(as_json_str_method(&lc.name, &readers));
        written.insert(lc.name.clone());
    }
    if written.is_empty() {
        return;
    }
    for controller in &mut app.controllers {
        for action in controller.actions_mut() {
            replace_in(&mut action.body, &mut |e| rewrite_render_json(e, &written));
        }
    }
}

/// `render json: <v>` → `render plain: <v>.as_json_str, content_type:
/// "application/json"` when `<v>` is typed as a class this pass wrote a
/// writer for. Other entries ride along untouched; an author's own
/// `content_type:` stands (the controller rewrite keeps the first).
fn rewrite_render_json(e: &Expr, written: &HashSet<ClassId>) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block, parenthesized } = &*e.node else {
        return None;
    };
    if method.as_str() != "render" || args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, kwargs: true } = &*args[0].node else { return None };
    let is_key = |k: &Expr, name: &str| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name)
    };
    let value = entries.iter().find_map(|(k, v)| is_key(k, "json").then_some(v))?;
    let Some(Ty::Class { id, .. }) = value.ty.as_ref() else { return None };
    if !written.contains(id) {
        return None;
    }
    let sym = |name: &str| {
        with_ty(
            Expr::new(value.span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } }),
            Ty::Sym,
        )
    };
    let encoded = with_ty(
        Expr::new(
            value.span,
            ExprNode::Send {
                recv: Some(value.clone()),
                method: Symbol::from(WRITER_METHOD),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
        Ty::Str,
    );
    let mut new_entries: Vec<(Expr, Expr)> = Vec::new();
    for (k, v) in entries {
        if is_key(k, "json") {
            new_entries.push((sym("plain"), encoded.clone()));
        } else {
            new_entries.push((k.clone(), v.clone()));
        }
    }
    if !entries.iter().any(|(k, _)| is_key(k, "content_type")) {
        new_entries.push((
            sym("content_type"),
            with_ty(
                Expr::new(
                    value.span,
                    ExprNode::Lit { value: Literal::Str { value: "application/json".to_string() } },
                ),
                Ty::Str,
            ),
        ));
    }
    let mut hash = args[0].clone();
    hash.node = Box::new(ExprNode::Hash { entries: new_entries, kwargs: true });
    Some(Expr {
        node: Box::new(ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: vec![hash],
            block: block.clone(),
            parenthesized: *parenthesized,
        }),
        ..e.clone()
    })
}

/// Post-order in-place replacement, the shape `params_merge` uses.
fn replace_in(expr: &mut Expr, f: &mut impl FnMut(&Expr) -> Option<Expr>) {
    expr.node.for_each_child_mut(&mut |c| replace_in(c, f));
    if let Some(replacement) = f(expr) {
        *expr = replacement;
    }
}

/// The writer over the same readers `as_json` answers: one
/// unconditional `Reader` pair per attribute, in declaration order. A
/// PORO has no table and no associations, so the writer never declines.
fn as_json_str_method(owner: &ClassId, readers: &[Symbol]) -> MethodDef {
    let pairs: Vec<JsonPair> = readers
        .iter()
        .map(|name| JsonPair { key: name.clone(), value: PairValue::Reader(name.clone()), cond: None })
        .collect();
    writer_method(owner, &pairs, None, &[]).expect("unconditional reader pairs always encode")
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
                {
                    // Stamped. This pass runs after the analyzer, so
                    // nothing types these reads afterwards, and an
                    // unstamped ivar is reported as `@title has no
                    // known type` — with no file or line, because the
                    // node is synthetic. `Untyped` is the honest
                    // answer: the readers here are `attr_*`
                    // declarations, which declare no type, and it is
                    // what the emitted RBS says of them.
                    let mut read =
                        Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() });
                    read.ty = Some(crate::ty::Ty::Untyped);
                    read
                },
            )
        })
        .collect();
    let body = Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: false });
    MethodDef {
        name_span: crate::span::Span::synthetic(),
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
