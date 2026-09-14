//! Lazy per-record state for the ruby family: has_many caches made on
//! first read, and one shared empty attrs Hash for the constructor's
//! default.
//!
//! The shared model lowering initialises every has_many association's
//! eager-load cache in `initialize` — `@memberships_cache = []` — so a
//! strict target's ivar is non-nilable (Crystal types an ivar nilable
//! unless every construction path assigns it). On Rust and Go that
//! empty vector costs nothing; on spinel and CRuby it is a heap Array
//! per association per record, whether or not the record's associations
//! are ever touched.
//!
//! Measured on campfire's room page (`scripts/alloc-window --sort
//! count`, one worker): `User#initialize` was 365 allocations per
//! request for 51 hydrated users — nine `_cache` Arrays each, for
//! records that exist only to answer `message.creator.name` — and on
//! the box at twelve workers `sp_PolyArray_new` was the single hottest
//! symbol (6.6%, another 4.4% recycling those arrays through the
//! per-class pool), with `User#initialize` its top caller.
//!
//! So on this emit path the cache starts `nil` and is made on first
//! read: every read of `@<assoc>_cache` outside `initialize` becomes a
//! call to a synthesized `__<assoc>_cache` that allocates once, and the
//! readers, targets and preload seeds are otherwise untouched. The
//! writers (`_preload_<assoc>(list)`, the collection setters) still
//! assign the ivar directly. spinel types the ivar `Array[T]?` from
//! its assignments and the getter's nil-check narrows it back, so every
//! caller keeps the `Array[T]` it had.
//!
//! The second allocation the same profile named: `def initialize(attrs
//! = {})`. Every hydration path constructs the record bare —
//! `Message.from_row` is `Message.new` then column writers — and the
//! `{}` default is a `Hash` per record that nothing writes (805
//! `Hash(Symbol)` per request, one per hydrated record). The default
//! becomes `EMPTY_ATTRS`, one frozen empty Hash on `ActiveRecord::Base`:
//! the constructor only reads `attrs`, and frozen means a write would
//! raise rather than corrupt every later record.
//!
//! Ruby-family only, applied at emit like `view_buffer_passing`: the
//! shared IR keeps the eager init the strict targets' typing wants.

use crate::app::App;
use crate::dialect::{Association, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

fn cache_ivar(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_cache", name.as_str()))
}

fn getter_name(name: &Symbol) -> Symbol {
    Symbol::from(format!("__{}_cache", name.as_str()))
}

fn var(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

/// `def __<assoc>_cache; c = @<assoc>_cache; if c.nil?; c = []; @<assoc>_cache = c; end; c; end`
fn getter(ivar: &Symbol, name: &Symbol) -> MethodDef {
    let span = Span::synthetic();
    let read = Expr::new(span, ExprNode::Ivar { name: ivar.clone() });
    let assign_c = Expr::new(
        span,
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: Symbol::from("c") }, value: read },
    );
    let nil_check = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(var("c")),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fresh = Expr::new(
        span,
        ExprNode::Array { elements: vec![], style: crate::expr::ArrayStyle::Brackets },
    );
    let c_fresh = Expr::new(
        span,
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: Symbol::from("c") }, value: fresh },
    );
    let ivar_c = Expr::new(
        span,
        ExprNode::Assign { target: LValue::Ivar { name: ivar.clone() }, value: var("c") },
    );
    let make = Expr::new(
        span,
        ExprNode::If {
            cond: nil_check,
            then_branch: Expr::new(span, ExprNode::Seq { exprs: vec![c_fresh, ivar_c] }),
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    MethodDef {
        name: getter_name(name),
        receiver: MethodReceiver::Instance,
        params: vec![],
        block_param: None,
        body: Expr::new(span, ExprNode::Seq { exprs: vec![assign_c, make, var("c")] }),
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: None,
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
    }
}

/// Every READ of `@<ivar>` becomes the getter call. Assignment targets
/// are `LValue`s, not `ExprNode::Ivar`, so they are untouched by
/// construction.
fn reads_to_getter(e: &mut Expr, ivar: &Symbol, getter: &Symbol) {
    if let ExprNode::Ivar { name } = &*e.node {
        if name == ivar {
            let span = e.span;
            let ty = e.ty.clone();
            *e = Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: getter.clone(),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            e.ty = ty;
            return;
        }
    }
    e.node.for_each_child_mut(&mut |c| reads_to_getter(c, ivar, getter));
}

/// `@<ivar> = []` in `initialize` → `@<ivar> = nil`.
fn init_to_nil(body: &mut Expr, ivar: &Symbol) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &mut *body.node {
        if name == ivar && matches!(&*value.node, ExprNode::Array { elements, .. } if elements.is_empty()) {
            *value = Expr::new(value.span, ExprNode::Lit { value: Literal::Nil });
            return;
        }
    }
    body.node.for_each_child_mut(&mut |c| init_to_nil(c, ivar));
}

/// `attrs = {}` → `attrs = ActiveRecord::Base::EMPTY_ATTRS` on a model's
/// `initialize`.
fn share_empty_attrs_default(m: &mut MethodDef) {
    if m.name.as_str() != "initialize" {
        return;
    }
    for p in m.params.iter_mut() {
        if p.name.as_str() != "attrs" {
            continue;
        }
        let Some(d) = &p.default else { continue };
        if !matches!(&*d.node, ExprNode::Hash { entries, .. } if entries.is_empty()) {
            continue;
        }
        p.default = Some(Expr::new(
            d.span,
            ExprNode::Const {
                path: vec![Symbol::from("ActiveRecord"), Symbol::from("Base"), Symbol::from("EMPTY_ATTRS")],
            },
        ));
    }
}

/// A class that `include ActiveModel::Model` is in `app.models` too, and
/// it gets the same `initialize(attrs = {})`; it is not an
/// `ActiveRecord::Base` and its file need not load one, so the shared
/// constant is not its to reference. It is built by callers with real
/// attrs (a form object), never bare by a hydration path, so it has
/// nothing to gain either.
fn includes_active_model_model(model: &crate::dialect::Model) -> bool {
    model.body.iter().any(|item| {
        let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return false };
        method.as_str() == "include"
            && args.iter().any(|a| matches!(&*a.node,
                ExprNode::Const { path } if path.len() == 2
                    && path[0].as_str() == "ActiveModel"
                    && path[1].as_str() == "Model"))
    })
}

pub fn apply(lcs: &mut [LibraryClass], app: &App) {
    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        if !includes_active_model_model(model) {
            for m in lc.methods.iter_mut() {
                share_empty_attrs_default(m);
            }
        }
        let names: Vec<Symbol> = model
            .associations()
            .into_iter()
            .filter_map(|a| match a {
                Association::HasMany { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        if names.is_empty() {
            continue;
        }
        let mut getters: Vec<MethodDef> = Vec::new();
        for name in &names {
            let ivar = cache_ivar(name);
            let g = getter_name(name);
            let mut had_init = false;
            for m in lc.methods.iter_mut() {
                if m.name.as_str() == "initialize" {
                    let before = format!("{:?}", m.body);
                    init_to_nil(&mut m.body, &ivar);
                    had_init |= before != format!("{:?}", m.body);
                } else {
                    reads_to_getter(&mut m.body, &ivar, &g);
                }
            }
            // Only where the eager init was there to replace: a model
            // whose initialize never assigned the cache is not one this
            // pass understands.
            if had_init {
                getters.push(getter(&ivar, name));
            }
        }
        lc.methods.extend(getters);
    }
}
