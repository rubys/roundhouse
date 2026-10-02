//! Builder wrappers are spliced into views, but their argument computations
//! still belong to the helper that defined them. Preserve that ownership with
//! small typed callable bridges; never expose the source's private methods.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodDef, MethodReceiver, MethodVisibility};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::{ParamKind, Ty};

/// A single builder-yielding call, with the caller's block forwarded through.
pub(crate) struct FormWrapperHelper {
    pub(super) params: Vec<(Symbol, Option<Expr>)>,
    pub(super) call: Expr,
}

fn wrapped_call(m: &MethodDef) -> Option<&Expr> {
    let blk = m.block_param.as_ref()?;
    if m.params.iter().any(|p| p.rest || p.keyword) {
        return None;
    }
    let body = match &*m.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => &m.body,
    };
    let ExprNode::Send { recv: None, method, block: Some(b), .. } = &*body.node else {
        return None;
    };
    if !matches!(method.as_str(), "form_with" | "form_for" | "fields_for")
        || !matches!(&*b.node, ExprNode::Var { name, .. } if *name == blk.name)
    {
        return None;
    }
    Some(body)
}

/// Resolve through the actual view-helper surface, not every library method
/// with a matching name. The form and its caller's builder block must be
/// visible together, so these one-call wrappers are substituted before the
/// view walk. Options-tail defaults and static merges are handled at the site.
pub(super) fn form_wrapper_helpers(app: &App) -> HashMap<String, FormWrapperHelper> {
    let mut out = HashMap::new();
    for lc in &app.library_classes {
        for m in &lc.methods {
            if app.helper_method_index.get(&m.name) != Some(&lc.name) {
                continue;
            }
            let Some(body) = wrapped_call(m) else { continue };
            let mut call = body.clone();
            if let ExprNode::Send { block, .. } = &mut *call.node {
                *block = None;
            }
            out.insert(
                m.name.as_str().to_string(),
                FormWrapperHelper {
                    params: m.params.iter().map(|p| (p.name.clone(), p.default.clone())).collect(),
                    call,
                },
            );
        }
    }
    out
}

/// Shared post-analyze synthesis. Deliberately limited to `data:` with
/// frame-independent computations and literal defaults. Its dynamic renderer
/// consumes the result once. Model/URL/namespace and id/class expression syntax
/// drives form classification/attribute rendering and must not be hidden behind
/// a bridge. Direct ivars still bind to the view's locals when spliced.
/// Executable defaults, captures, assignments
/// and control-flow need a real wrapper-frame lowering, not this substitution.
/// Existing argument/default/merge evaluation limitations are not expanded here.
pub(crate) fn preserve_argument_owners(app: &mut App) {
    let mut names: HashMap<_, HashSet<_>> = HashMap::new();
    let mut counts: HashMap<_, usize> = HashMap::new();
    for lc in &app.library_classes {
        for m in &lc.methods {
            names.entry(lc.name.clone()).or_default().insert(m.name.clone());
            *counts.entry((lc.name.clone(), m.name.clone())).or_default() += 1;
        }
    }
    for lc in &mut app.library_classes {
        if !lc.is_module
            || lc.methods.is_empty()
            || app.controllers.iter().any(|c| c.name == lc.name)
        {
            continue;
        }
        let own_names = names[&lc.name].clone();
        let allocated = names.get_mut(&lc.name).expect("collected owner");
        let mut added = Vec::new();
        for m in &mut lc.methods {
            if app.helper_method_index.get(&m.name) != Some(&lc.name)
                || counts[&(lc.name.clone(), m.name.clone())] != 1
                || m.params.iter().any(|p| p.default.as_ref().is_some_and(|e| !literal_default(e)))
            {
                continue;
            }
            let Some(call) = wrapped_call(m) else { continue };
            let ExprNode::Send { args, .. } = &*call.node else { unreachable!() };
            let mut args = args.clone();
            for arg in &mut args {
                let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
                for (key, value) in entries {
                    if !matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } }
                        if value.as_str() == "data")
                    {
                        continue;
                    }
                    let mut reads = HashSet::new();
                    let mut needs_owner = false;
                    let mut effects = EffectSet::pure();
                    if !inspect_argument(
                        value,
                        &own_names,
                        &mut reads,
                        &mut needs_owner,
                        &mut effects,
                    ) || !needs_owner
                        || reads.iter().any(|name| !m.params.iter().any(|p| &p.name == name))
                    {
                        continue;
                    }
                    let Some(ret) = value.ty.clone() else { continue };
                    let Some(Ty::Fn { params: sig_params, .. }) = &m.signature else { continue };
                    let mut bridge = m.clone();
                    bridge.params.retain(|p| reads.contains(&p.name));
                    let mut params = Vec::new();
                    for p in &mut bridge.params {
                        p.default = None;
                        p.from_keyword = false;
                        p.from_kwrest = false;
                        let Some(sig) = sig_params.iter().find(|sig| sig.name == p.name) else {
                            continue;
                        };
                        let mut sig = sig.clone();
                        sig.kind = ParamKind::Required;
                        params.push(sig);
                    }
                    if params.len() != bridge.params.len() {
                        continue;
                    }
                    // Ruby permits !/? only at the end of a method name. A
                    // generated ordinal must not turn a valid wrapper into
                    // an invalid def; collisions are checked after normalizing.
                    let stem = m.name.as_str().trim_end_matches(['!', '?', '=']);
                    let mut ordinal = 0;
                    let name = loop {
                        let name = Symbol::from(format!("__rh_form_{stem}_{ordinal}"));
                        if allocated.insert(name.clone()) {
                            break name;
                        }
                        ordinal += 1;
                    };
                    bridge.name = name.clone();
                    bridge.name_span = Span::synthetic();
                    bridge.receiver = MethodReceiver::Class;
                    bridge.visibility = MethodVisibility::Public;
                    bridge.block_param = None;
                    bridge.body = value.clone();
                    bridge.effects = effects.clone();
                    let call_args = params
                        .iter()
                        .map(|p| {
                            let mut arg = Expr::new(
                                value.span,
                                ExprNode::Var { id: VarId(0), name: p.name.clone() },
                            );
                            arg.ty = Some(p.ty.clone());
                            arg
                        })
                        .collect();
                    bridge.signature = Some(Ty::Fn {
                        params,
                        block: None,
                        ret: Box::new(ret),
                        effects: effects.clone(),
                    });
                    let recv = Expr::new(
                        value.span,
                        ExprNode::Const {
                            path: lc.name.0.as_str().split("::").map(Symbol::from).collect(),
                        },
                    );
                    value.node = Box::new(ExprNode::Send {
                        recv: Some(recv),
                        method: name,
                        args: call_args,
                        block: None,
                        parenthesized: true,
                    });
                    value.effects = effects;
                    added.push(bridge);
                }
            }
            let body = match &mut *m.body.node {
                ExprNode::Seq { exprs } if exprs.len() == 1 => &mut exprs[0],
                _ => &mut m.body,
            };
            if let ExprNode::Send { args: target, .. } = &mut *body.node {
                *target = args;
            }
        }
        lc.methods.extend(added);
    }
}

fn literal_default(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Array { elements, .. } => elements.iter().all(literal_default),
        ExprNode::Hash { entries, .. } => {
            entries.iter().all(|(k, v)| literal_default(k) && literal_default(v))
        }
        _ => false,
    }
}

fn inspect_argument(
    e: &Expr,
    own_names: &HashSet<Symbol>,
    reads: &mut HashSet<Symbol>,
    needs_owner: &mut bool,
    effects: &mut EffectSet,
) -> bool {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            reads.insert(name.clone());
        }
        ExprNode::Send { recv, method, block: None, .. } => {
            *needs_owner |= recv.is_none() && own_names.contains(method);
        }
        ExprNode::Lit { .. }
        | ExprNode::Const { .. }
        | ExprNode::Hash { .. }
        | ExprNode::Array { .. }
        | ExprNode::StringInterp { .. } => {}
        _ => return false,
    }
    effects.effects.extend(e.effects.effects.iter().cloned());
    let mut valid = true;
    e.node.for_each_child(&mut |child| {
        valid &= inspect_argument(child, own_names, reads, needs_owner, effects);
    });
    valid
}
