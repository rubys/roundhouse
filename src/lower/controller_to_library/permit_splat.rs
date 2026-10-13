//! `permit(*helper, …)` where `helper` is a method of the controller
//! (or one it inherits) that answers a literal Symbol list.
//!
//! ```ruby
//! def common_fields = %i[name color]
//! def gadget_params = params.require(:gadget).permit(*common_fields, :size)
//! ```
//!
//! The permit recognizer reads Symbols, one Array literal and a
//! trailing keyword Hash; a splat fails its match, and the whole chain
//! was emitted as written, reaching `Kernel#require` on the request's
//! plain Hash. Splatting a list that is a literal in the source is the
//! same permit as writing its Symbols inline, so they are written
//! inline here, before anything reads the permit lists.
//!
//! Only a call with no receiver (or `self`) and no arguments, to a
//! method with no parameters whose body is a Symbol Array literal
//! (`%i[...]`, `[:a, :b]`, optionally `.freeze`). Anything else under
//! the splat is left alone.

use std::collections::HashMap;

use crate::dialect::{Action, Controller};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

/// The controllers with every recognized splat written inline, or None
/// when none has one (the common case: nothing is cloned).
pub(super) fn inline_permit_splats(controllers: &[Controller]) -> Option<Vec<Controller>> {
    let by_name: HashMap<&ClassId, &Controller> = controllers.iter().map(|c| (&c.name, c)).collect();
    let mut out: Option<Vec<Controller>> = None;
    for (i, controller) in controllers.iter().enumerate() {
        let lists = |name: &Symbol| symbol_list_of(controller, name, &by_name);
        let mut changed = false;
        let mut actions: Vec<Action> = controller.actions().cloned().collect();
        for action in &mut actions {
            changed |= rewrite(&mut action.body, &lists);
        }
        if !changed {
            continue;
        }
        let owned = out.get_or_insert_with(|| controllers.to_vec());
        for (slot, action) in owned[i].actions_mut().zip(actions) {
            *slot = action;
        }
    }
    out
}

/// The Symbol list a no-argument method `name` answers, looked up in
/// `controller` first and then up its superclass chain.
fn symbol_list_of(
    controller: &Controller,
    name: &Symbol,
    by_name: &HashMap<&ClassId, &Controller>,
) -> Option<Vec<Expr>> {
    let mut current = Some(controller);
    let mut seen = 0;
    while let Some(c) = current {
        if let Some(def) = c.actions().filter(|a| &a.name == name).last() {
            if !def.formal_params().is_empty() || def.block_param.is_some() {
                return None;
            }
            return literal_symbols(&def.body);
        }
        seen += 1;
        if seen > 32 {
            return None;
        }
        current = c.parent.as_ref().and_then(|p| by_name.get(p).copied());
    }
    None
}

fn literal_symbols(body: &Expr) -> Option<Vec<Expr>> {
    let body = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let array = match &*body.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "freeze" && args.is_empty() =>
        {
            r
        }
        _ => body,
    };
    let ExprNode::Array { elements, .. } = &*array.node else {
        return None;
    };
    let all_symbols = !elements.is_empty()
        && elements.iter().all(|e| matches!(&*e.node, ExprNode::Lit { value: Literal::Sym { .. } }));
    all_symbols.then(|| elements.clone())
}

/// Rewrites every `permit(…, *helper, …)` under `expr`; answers whether
/// one was.
fn rewrite(expr: &mut Expr, lists: &impl Fn(&Symbol) -> Option<Vec<Expr>>) -> bool {
    let mut changed = false;
    expr.node.for_each_child_mut(&mut |child| changed |= rewrite(child, lists));
    let ExprNode::Send { method, args, .. } = &mut *expr.node else {
        return changed;
    };
    if method.as_str() != "permit" {
        return changed;
    }
    let mut expanded = Vec::with_capacity(args.len());
    let mut any = false;
    for arg in args.iter() {
        if let ExprNode::Splat { value } = &*arg.node
            && let ExprNode::Send { recv, method: helper, args: helper_args, block: None, .. } = &*value.node
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && helper_args.is_empty()
            && let Some(symbols) = lists(helper)
        {
            expanded.extend(symbols);
            any = true;
            continue;
        }
        expanded.push(arg.clone());
    }
    if any {
        *args = expanded;
    }
    changed || any
}
