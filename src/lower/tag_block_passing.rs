//! Buffer-passing variants for block-taking TAG HELPERS.
//!
//! [`super::view_buffer_passing`] stops a nested VIEW from crossing the
//! string seam: `io << Views::X.y(a)` becomes `Views::X.y_into(io, a)`.
//! It left one shape untouched, and on campfire's room page that shape
//! is where most of the bytes go: a helper written as
//!
//! ```ruby
//! def messages_tag(room, &)
//!   tag.div id: dom_id(room, :messages), …, &
//! end
//! ```
//!
//! which `tag_builder` lowers to ONE interpolation —
//! `"<div …>#{capture(&__blk)}</div>"` — and which a view calls as
//!
//! ```ruby
//! io << MessagesHelper.messages_tag(room) do
//!   _cap = String.new
//!   messages.each { |m| Views::Messages.message_into(_cap, m, nil, nil) }
//!   _cap
//! end
//! ```
//!
//! Measured (`scripts/alloc-window`, `/rooms/1`, one worker): the 423 KB
//! message list is built in `_cap`, copied once into the interpolation
//! that wraps it in the tag, and copied again as `io <<` grows the
//! caller's buffer by a page — and the room page nests two of these
//! (`message_area_tag` around `messages_tag`), so four page-sized copies
//! of the 4.5 MB a request allocates are this seam, on top of the
//! doubling growth each `_cap` pays on its own.
//!
//! So the seam is closed the same way, in two halves. Each such helper
//! gains a `<name>_open_into(io, …)` variant that writes the OPEN tag
//! into the caller's buffer and returns; and a view's append of a block
//! call becomes that call, then the block's body appending straight
//! into the view's own buffer, then the CLOSE tag appended by the view
//! itself (a literal — it is the helper's own suffix, read off the same
//! lowered shape the variant is built from; a suffix with an expression
//! in it gets a `<name>_close_into` instead). The original helper is
//! untouched, so every caller that wants a string (a controller, a
//! broadcast, a helper composing helpers) still has it.
//!
//! WHY NOT ONE VARIANT THAT YIELDS. That was the first shape, and it is
//! the natural one; spinel's by-reference rules for the buffer refuse
//! it three ways, each filed with a reduction:
//! * handing a String parameter to a `&blk`-taking callee drops the
//!   by-reference ABI for the WHOLE caller (matz/spinel#4477);
//! * a method whose only use of the buffer is passing it to a yielding
//!   callee is not by-reference either (matz/spinel#4476);
//! * two nested inlined yielding callees lose the outer one's append
//!   after its yield (matz/spinel#4479) — which is exactly the room
//!   page's `message_area_tag` around `messages_tag`.
//! A non-yielding callee that appends is the shape the view variants
//! already rely on, so the open tag goes through one of those and no
//! block ever crosses a method boundary. The gate below (a view is only
//! rewritten when its body appends to the buffer directly) is kept as
//! belt and braces: it costs nothing and every real template passes.
//!
//! Ruby-family only, like its sibling: the construct needs a mutable
//! buffer a callee can write through.

use std::collections::HashMap;

use crate::app::App;
use crate::dialect::{LibraryClass, MethodDef, Param};
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::{ParamKind, Ty};

use super::view_buffer_passing::INTO_SUFFIX;

/// What a view needs to know about a qualifying helper: its positional
/// arity and its close tag when that is a literal (`None` means call
/// the `_close_into` variant).
pub struct Variant {
    pub arity: usize,
    pub close: Option<String>,
}

/// The helpers that qualify, keyed `(module, method)`. Computed from
/// the App so the helper emit (which adds the variants) and the view
/// emit (which calls them) agree without one having to run before the
/// other.
pub fn variants(app: &App) -> HashMap<(String, String), Variant> {
    let mut out = HashMap::new();
    for lc in &app.library_classes {
        for m in &lc.methods {
            if let Some((parts, idx)) = tag_block_shape(m) {
                let h = halves(&parts, idx);
                out.insert(
                    (lc.name.0.to_string(), m.name.as_str().to_string()),
                    Variant { arity: m.params.len(), close: close_literal(&h.close) },
                );
            }
        }
    }
    out
}

/// The tag-builder shape: a method with a named block parameter, no
/// keyword/rest/default parameters (the variant's must all be required
/// — `view_buffer_passing::split` says why), whose
/// body is a single string interpolation (bare, or under `.html_safe`)
/// with exactly one part that captures the block. Answers the
/// interpolation's parts and the index of that part.
fn tag_block_shape(m: &MethodDef) -> Option<(Vec<InterpPart>, usize)> {
    let blk = m.block_param.as_ref()?;
    if m.params.iter().any(|p| p.keyword || p.rest || p.default.is_some()) {
        return None;
    }
    let mut body = &m.body;
    if let ExprNode::Seq { exprs } = &*body.node {
        if exprs.len() != 1 {
            return None;
        }
        body = &exprs[0];
    }
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*body.node {
        if method.as_str() == "html_safe" && args.is_empty() {
            body = r;
        }
    }
    let ExprNode::StringInterp { parts } = &*body.node else { return None };
    let captures: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p, InterpPart::Expr { expr } if is_capture_of(expr, &blk.name)))
        .map(|(i, _)| i)
        .collect();
    let [idx] = captures.as_slice() else { return None };
    Some((parts.clone(), *idx))
}

/// `capture(&blk)`, bare or under the `blk.nil? ? "" : …` guard
/// `tag_builder::capture_call` writes for a forwarded block.
fn is_capture_of(e: &Expr, blk: &Symbol) -> bool {
    match &*e.node {
        ExprNode::Send { recv: None, method, args, block: Some(b), .. } => {
            method.as_str() == "capture"
                && args.is_empty()
                && matches!(&*b.node, ExprNode::Var { name, .. } if name == blk)
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            matches!(&*cond.node, ExprNode::Send { recv: Some(r), method, .. }
                if method.as_str() == "nil?" && matches!(&*r.node, ExprNode::Var { name, .. } if name == blk))
                && matches!(&*then_branch.node, ExprNode::Lit { value: Literal::Str { value } } if value.is_empty())
                && is_capture_of(else_branch, blk)
        }
        _ => false,
    }
}

fn var(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

fn append(acc: &str, value: Expr) -> Expr {
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var(acc)),
            method: Symbol::from("<<"),
            args: vec![value],
            block: None,
            parenthesized: false,
        },
    );
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

/// The interpolation's parts as one appendable value: a single text
/// part is the literal itself, anything else stays an interpolation.
fn interp_value(parts: Vec<InterpPart>) -> Option<Expr> {
    match parts.as_slice() {
        [] => None,
        [InterpPart::Text { value }] => Some(Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: value.clone() } },
        )),
        _ => Some(Expr::new(Span::synthetic(), ExprNode::StringInterp { parts })),
    }
}

/// The two halves of a helper's interpolation around its capture.
struct Halves {
    open: Vec<InterpPart>,
    close: Vec<InterpPart>,
}

fn halves(parts: &[InterpPart], idx: usize) -> Halves {
    Halves { open: parts[..idx].to_vec(), close: parts[idx + 1..].to_vec() }
}

/// The close half as the literal a view can append itself, when it is
/// one — `</div>` — rather than an expression.
fn close_literal(close: &[InterpPart]) -> Option<String> {
    match close {
        [] => Some(String::new()),
        [InterpPart::Text { value }] => Some(value.clone()),
        _ => None,
    }
}

/// `<name>_<half>_into(io, <params>)`: that half's parts appended to
/// `io`, then nil. A non-yielding method that appends to its String
/// parameter — the shape spinel keeps by-reference.
fn half_variant(m: &MethodDef, half: &str, parts: Vec<InterpPart>) -> MethodDef {
    const ACC: &str = "io";
    let mut body: Vec<Expr> = Vec::new();
    if let Some(v) = interp_value(parts) {
        body.push(append(ACC, v));
    }
    body.push(nil_lit());

    let mut params = vec![Param {
        name: Symbol::from(ACC),
        default: None,
        keyword: false,
        rest: false,
        from_keyword: false,
        from_kwrest: false,
    }];
    params.extend(m.params.iter().cloned());
    // `io` declared `String` for the by-reference ABI, the rest
    // `untyped` — the same reasoning as `view_buffer_passing::split`.
    let signature = match &m.signature {
        Some(Ty::Fn { params: ps, block, effects, .. }) => {
            let mut out = vec![crate::ty::Param {
                name: Symbol::from(ACC),
                ty: Ty::Str,
                kind: ParamKind::Required,
            }];
            out.extend(ps.iter().map(|p| crate::ty::Param {
                name: p.name.clone(),
                ty: Ty::Untyped,
                kind: ParamKind::Required,
            }));
            Some(Ty::Fn { params: out, block: block.clone(), ret: Box::new(Ty::Nil), effects: effects.clone() })
        }
        _ => None,
    };
    let mut v = m.clone();
    v.name = Symbol::from(half_name(m.name.as_str(), half).as_str());
    v.params = params;
    v.block_param = None;
    v.signature = signature;
    let mut vbody = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: body });
    vbody.inherit_span(m.body.span);
    v.body = vbody;
    v
}

fn half_name(method: &str, half: &str) -> String {
    format!("{method}_{half}{INTO_SUFFIX}")
}

/// Add the variants to the helper modules that define them: the open
/// half always, the close half only when it is not a literal the view
/// appends itself. The original methods stay.
pub fn apply_helpers(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        let mut added: Vec<MethodDef> = Vec::new();
        for m in &lc.methods {
            if let Some((parts, idx)) = tag_block_shape(m) {
                let h = halves(&parts, idx);
                added.push(half_variant(m, "open", h.open));
                if close_literal(&h.close).is_none() {
                    added.push(half_variant(m, "close", h.close));
                }
            }
        }
        lc.methods.extend(added);
    }
}

/// Does a method body append to `acc` at its top level — outside any
/// block? The by-reference gate (module header, matz/spinel#4476).
fn appends_directly(body: &Expr, acc: &str) -> bool {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    stmts.iter().any(|s| {
        matches!(&*s.node, ExprNode::Send { recv: Some(r), method, .. }
            if method.as_str() == "<<" && matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == acc))
    })
}

/// Replace every read of local `from` with a read of `to`.
fn rename_var(e: &mut Expr, from: &str, to: &str) {
    if let ExprNode::Var { name, .. } = &mut *e.node {
        if name.as_str() == from {
            *name = Symbol::from(to);
        }
    }
    e.node.for_each_child_mut(&mut |c| rename_var(c, from, to));
}

/// The capture block the view walker builds: `_capN = String.new;
/// …; _capN`. Answers the accumulator's name.
fn capture_block_acc(body: &Expr) -> Option<String> {
    let ExprNode::Seq { exprs } = &*body.node else { return None };
    if exprs.len() < 2 {
        return None;
    }
    let (first, last) = (&exprs[0], &exprs[exprs.len() - 1]);
    if first.hint != Some(IrHint::StringBuilderInit) || last.hint != Some(IrHint::StringBuilderResult) {
        return None;
    }
    let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*first.node else { return None };
    let ExprNode::Var { name: tail, .. } = &*last.node else { return None };
    (name == tail).then(|| name.as_str().to_string())
}

/// `acc << Mod.m(args) do _cap = String.new; …; _cap end` →
/// `Mod.m_open_into(acc, args); …; acc << "</tag>"` — the block's body
/// in statement position with its `_cap` renamed to `acc`. Recurses
/// into the rewritten body afterwards, so a block nested inside a
/// block (the room page's two) is rewritten against the same buffer.
fn rewrite_appends(e: &mut Expr, acc: &str, variants: &HashMap<(String, String), Variant>) {
    let rewritten = try_rewrite(e, acc, variants);
    if !rewritten {
        e.node.for_each_child_mut(&mut |c| rewrite_appends(c, acc, variants));
    }
}

fn try_rewrite(e: &mut Expr, acc: &str, variants: &HashMap<(String, String), Variant>) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else { return false };
    if method.as_str() != "<<" || args.len() != 1 {
        return false;
    }
    if !matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == acc) {
        return false;
    }
    let ExprNode::Send { recv: Some(callee_recv), method: callee, args: cargs, block: Some(blk), .. } =
        &*args[0].node
    else {
        return false;
    };
    let ExprNode::Const { path } = &*callee_recv.node else { return false };
    // A rooted receiver (`::Rooms::InvolvementsHelper`, from
    // `apply_constant_rooting`) names the same module as the App's
    // `Rooms::InvolvementsHelper`.
    let key = (
        path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"),
        callee.as_str().to_string(),
    );
    let Some(variant) = variants.get(&key) else { return false };
    if cargs.len() != variant.arity {
        return false;
    }
    let ExprNode::Lambda { params: bparams, rest_param, block_param, body, .. } = &*blk.node else {
        return false;
    };
    if !bparams.is_empty() || rest_param.is_some() || block_param.is_some() {
        return false;
    }
    let Some(cap) = capture_block_acc(body) else { return false };

    let ExprNode::Seq { exprs } = &*body.node else { return false };
    let mut inner: Vec<Expr> = exprs[1..exprs.len() - 1].to_vec();
    for stmt in inner.iter_mut() {
        rename_var(stmt, &cap, acc);
    }
    let mut inner_seq = Expr::new(body.span, ExprNode::Seq { exprs: inner });
    rewrite_appends(&mut inner_seq, acc, variants);
    let ExprNode::Seq { exprs: inner } = *inner_seq.node else { unreachable!() };

    let mut into_args = vec![var(acc)];
    into_args.extend(cargs.iter().cloned());
    let half_call = |half: &str| {
        Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(callee_recv.clone()),
                method: Symbol::from(half_name(callee.as_str(), half).as_str()),
                args: into_args.clone(),
                block: None,
                parenthesized: true,
            },
        )
    };
    let mut stmts = vec![half_call("open")];
    stmts.extend(inner);
    match &variant.close {
        Some(lit) if lit.is_empty() => {}
        Some(lit) => stmts.push(append(
            acc,
            Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: lit.clone() } }),
        )),
        None => stmts.push(half_call("close")),
    }
    // A Seq in statement position: the ruby emitter prints its
    // statements in place.
    *e = Expr::new(e.span, ExprNode::Seq { exprs: stmts });
    true
}

/// Rewrite the view modules' appends of block helper calls. Runs after
/// `view_buffer_passing::apply`, so a view's own `_into` variant is what
/// carries the buffer parameter (`io`) the gate checks.
pub fn apply_views(lcs: &mut [LibraryClass], variants: &HashMap<(String, String), Variant>) {
    if variants.is_empty() {
        return;
    }
    for lc in lcs.iter_mut() {
        for m in lc.methods.iter_mut() {
            // The buffer is the first parameter of an `_into` variant
            // or the accumulator local a returning view initialises —
            // not whichever local the first `<<` happens to name (the
            // room page's first append is to a `content_for` capture).
            let Some(acc) = buffer_name(m) else { continue };
            if !appends_directly(&m.body, &acc) {
                continue;
            }
            rewrite_appends(&mut m.body, &acc, variants);
        }
    }
}

/// The buffer a view method writes: its first parameter when it is a
/// `_into` variant, else the local its `StringBuilderInit` statement
/// assigns.
fn buffer_name(m: &MethodDef) -> Option<String> {
    if m.name.as_str().ends_with(INTO_SUFFIX) {
        return m.params.first().map(|p| p.name.as_str().to_string());
    }
    let ExprNode::Seq { exprs } = &*m.body.node else { return None };
    let first = exprs.first()?;
    if first.hint != Some(IrHint::StringBuilderInit) {
        return None;
    }
    let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*first.node else { return None };
    Some(name.as_str().to_string())
}
