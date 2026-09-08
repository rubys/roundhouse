//! Buffer-passing variants for lowered view methods.
//!
//! `view_to_library` gives every view the accumulator shape
//! (`io = String.new; io << …; io`), which is the right way to BUILD a
//! string on every target. What it costs is at the seam: a nested view
//! returns its finished buffer as a string value and its caller appends
//! that value into its own buffer, so a page pays one full copy of each
//! fragment per nesting level it passes through.
//!
//! On spinel that copy is not a duplicate that a peephole could drop —
//! the accumulator's storage is malloc'd and owned by the buffer handle
//! (`sp_String`), so handing it out as a string value is a change of
//! representation and of owner. Measured on the campfire port, warm,
//! two allocation reports 100 `/rooms/1` requests apart: 11.5 MB
//! allocated per request, 9.6 MB of it (83%) in 2,608 of those copies
//! averaging 3.7 KB, for a 423 KB page — about twenty-three copies of
//! the page per request. A reduction with identical output at four
//! nesting depths allocates output x depth, exactly.
//!
//! So the fix is to stop crossing the seam: each view gains a
//! `<name>_into(io, …)` variant that appends into the CALLER's buffer,
//! and a view-to-view append (`io << Views::Messages.message(m)`) calls
//! that instead. The original method stays, wrapping the variant, so
//! every caller that genuinely wants a string — a controller rendering
//! a page, a model broadcasting a fragment, a cache block storing one —
//! is untouched and keeps working.
//!
//! Applied on the Ruby-family emit path only. A target whose strings
//! are immutable (elixir) cannot express "append into the caller's
//! buffer" at all, and lowering it for everyone would put a construct
//! in the shared IR that the weakest emitter cannot spell.

use std::collections::HashMap;

use crate::dialect::{LibraryClass, MethodDef, Param};
use crate::expr::{Expr, ExprNode, IrHint, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::{ParamKind, Ty};

/// Suffix for the buffer-taking variant. `_into` reads as the Ruby
/// convention for "write into what I hand you" (`Marshal.dump(obj, io)`
/// spells the same idea positionally).
pub(crate) const INTO_SUFFIX: &str = "_into";

/// The variant's name is the method's plus the suffix. It does NOT need
/// to be unique program-wide: spinel decides the by-reference ABI — the
/// one that makes a callee's append visible to its caller — per NAME
/// GROUP since matz/spinel#4390, so `show_into` in nine view modules
/// takes it or none of them do, which is the property this pass needs.
/// Before that fix it was keyed on the bare name being unique, and 62 of
/// 144 variants qualified: the room page came out at 27,815 bytes
/// instead of 423,364, silently.
fn into_name(_module_path: &[Symbol], method: &str) -> String {
    format!("{method}{INTO_SUFFIX}")
}

/// The accumulator triple a lowered view body is built as. Returns the
/// accumulator's name when the body has exactly that shape: a first
/// statement tagged [`IrHint::StringBuilderInit`] assigning a local, and
/// a last statement tagged [`IrHint::StringBuilderResult`] reading the
/// same local back. Anything else — a jbuilder body, a hand-written
/// helper that found its way into a view module — is left alone.
fn accumulator_name(m: &MethodDef) -> Option<String> {
    let ExprNode::Seq { exprs } = &*m.body.node else { return None };
    let (first, last) = (exprs.first()?, exprs.last()?);
    if exprs.len() < 2 {
        return None;
    }
    if first.hint != Some(IrHint::StringBuilderInit)
        || last.hint != Some(IrHint::StringBuilderResult)
    {
        return None;
    }
    let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*first.node else {
        return None;
    };
    let ExprNode::Var { name: tail, .. } = &*last.node else { return None };
    (name == tail).then(|| name.as_str().to_string())
}

/// Keyword and rest parameters are not forwarded by this slice: the
/// wrapper would have to rebuild the caller's keyword bundle, and a
/// view that takes strict locals is rare enough that keeping it on the
/// returning shape costs one copy of that fragment rather than a class
/// of forwarding bugs.
fn forwardable(m: &MethodDef) -> bool {
    m.params.iter().all(|p| !p.keyword && !p.rest)
}

fn var(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

/// `Views::X.<method>(<args>)` with an explicit constant receiver, the
/// same shape `partial.rs` builds its render calls with.
fn const_call(path: &[Symbol], method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: path.to_vec() },
            )),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// Split one view method into a buffer-taking variant plus a wrapper
/// that allocates a buffer, calls it and returns the string. The
/// wrapper keeps the original name, arity and signature, so no call
/// site outside the view layer changes.
fn split(m: &MethodDef, module_path: &[Symbol], acc: &str) -> (MethodDef, MethodDef) {
    let ExprNode::Seq { exprs } = &*m.body.node else { unreachable!("checked by accumulator_name") };

    // The variant: the body without its buffer init and without its
    // trailing read. The read is what materialises the buffer, so
    // leaving it off is the entire point; `nil` takes its place rather
    // than whatever the last statement happens to evaluate to, so the
    // method has one obvious return type and no caller is tempted to
    // use it.
    let mut into_body: Vec<Expr> = exprs[1..exprs.len() - 1].to_vec();
    into_body.push(nil_lit());
    let mut into_params = vec![Param {
        name: Symbol::from(acc),
        default: None,
        keyword: false,
        rest: false,
        from_keyword: false,
        from_kwrest: false,
    }];
    // The variant's parameters are all REQUIRED, defaults and all: the
    // wrapper passes every argument explicitly and so does a rewritten
    // append, so nothing is ever omitted, and spinel gives the
    // by-reference parameter ABI (the one that makes an append visible
    // to the caller) only to methods whose parameters are all required
    // positional. Keeping an unused `= nil` here would leave the
    // variant on value semantics and its appends would land in a copy.
    into_params.extend(m.params.iter().map(|p| Param { default: None, ..p.clone() }));

    // `io` is declared, everything else is `untyped`. Declaring `io` is
    // what keeps spinel's by-reference parameter ABI — the mechanism
    // that makes the callee's appends visible to the caller — and an
    // `untyped` there would put the parameter back on value semantics
    // and lose them. Not carrying the wrapper's other declared types is
    // deliberate: a partial's extra local is typed `String?` by the view
    // lowerer whatever the caller passes, which is inert while the
    // parameter is optional and a type error once it is required (a
    // `Message` handed to `?String? message`). The wrapper still
    // declares the public shape; this one is an internal ABI.
    let into_signature = match &m.signature {
        Some(Ty::Fn { params, block, effects, .. }) => {
            let mut ps = vec![crate::ty::Param {
                name: Symbol::from(acc),
                ty: Ty::Str,
                kind: ParamKind::Required,
            }];
            ps.extend(params.iter().map(|p| crate::ty::Param {
                name: p.name.clone(),
                ty: Ty::Untyped,
                kind: ParamKind::Required,
            }));
            Some(Ty::Fn {
                params: ps,
                block: block.clone(),
                ret: Box::new(Ty::Nil),
                effects: effects.clone(),
            })
        }
        _ => None,
    };

    let into_name = into_name(module_path, m.name.as_str());
    let mut variant = m.clone();
    variant.name = Symbol::from(into_name.as_str());
    variant.params = into_params;
    variant.signature = into_signature;
    let mut vbody = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: into_body });
    vbody.inherit_span(m.body.span);
    variant.body = vbody;

    // The wrapper: the accumulator triple it always had, with one call
    // in the middle. Keeps the hints, so the Crystal/Go/TS accumulator
    // forms still apply to it if this pass is ever run for them.
    let mut init = exprs[0].clone();
    init.hint = Some(IrHint::StringBuilderInit);
    let mut args = vec![var(acc)];
    args.extend(m.params.iter().map(|p| var(p.name.as_str())));
    let mut tail = var(acc);
    tail.hint = Some(IrHint::StringBuilderResult);
    let mut wrapper = m.clone();
    let mut wbody = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![init, const_call(module_path, &into_name, args), tail] },
    );
    wbody.inherit_span(m.body.span);
    wrapper.body = wbody;

    (variant, wrapper)
}

/// `acc << Views::X.y(a)` → `Views::X.y_into(acc, a)`, wherever the
/// appended value is a call to a view that has a variant. Every other
/// append is left exactly as it was: an append of a helper result, of a
/// literal, or of a view call nested inside another call still
/// materialises, because those values are consumed by something other
/// than this buffer.
fn rewrite_appends(e: &mut Expr, variants: &HashMap<(String, String), Vec<Option<Expr>>>) {
    e.node.for_each_child_mut(&mut |c| rewrite_appends(c, variants));
    let ExprNode::Send { recv: Some(acc), method, args, block: None, .. } = &*e.node else {
        return;
    };
    if method.as_str() != "<<" || args.len() != 1 {
        return;
    }
    let ExprNode::Send {
        recv: Some(callee_recv),
        method: callee,
        args: callee_args,
        block: None,
        ..
    } = &*args[0].node
    else {
        return;
    };
    let ExprNode::Const { path } = &*callee_recv.node else { return };
    let key = (
        path.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>().join("::"),
        callee.as_str().to_string(),
    );
    let Some(defaults) = variants.get(&key) else { return };
    // The variant's parameters are all required (see `split`), so a call
    // that relied on the wrapper's defaults has to spell them: `render
    // "message", message: m` reaches `message(m)` and must reach
    // `message_into(io, m, nil, nil)`. A default that is not a literal
    // the caller could have written is left alone — the append keeps
    // the returning form rather than guessing.
    if callee_args.len() > defaults.len() {
        return;
    }
    let mut into_args = vec![acc.clone()];
    into_args.extend(callee_args.iter().cloned());
    for d in &defaults[callee_args.len()..] {
        match d {
            Some(expr) => into_args.push(expr.clone()),
            None => return,
        }
    }
    let into_name = into_name(path, callee.as_str());
    let mut call = const_call(path, &into_name, into_args);
    call.span = e.span;
    *e = call;
}

/// Give every convertible view method a buffer-taking variant and point
/// view-to-view appends at it.
pub fn apply(lcs: &mut [LibraryClass]) {
    let mut variants: HashMap<(String, String), Vec<Option<Expr>>> = HashMap::new();
    for lc in lcs.iter() {
        for m in &lc.methods {
            if accumulator_name(m).is_some() && forwardable(m) {
                variants.insert(
                    (lc.name.0.to_string(), m.name.as_str().to_string()),
                    m.params.iter().map(|p| p.default.clone()).collect(),
                );
            }
        }
    }
    if variants.is_empty() {
        return;
    }
    for lc in lcs.iter_mut() {
        let module_path: Vec<Symbol> =
            lc.name.0.as_str().split("::").map(Symbol::from).collect();
        let mut out: Vec<MethodDef> = Vec::with_capacity(lc.methods.len() * 2);
        for m in lc.methods.drain(..) {
            match accumulator_name(&m) {
                Some(acc) if forwardable(&m) => {
                    let (variant, wrapper) = split(&m, &module_path, &acc);
                    out.push(variant);
                    out.push(wrapper);
                }
                _ => out.push(m),
            }
        }
        lc.methods = out;
        for m in lc.methods.iter_mut() {
            rewrite_appends(&mut m.body, &variants);
        }
    }
}
