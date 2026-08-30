//! `GlobalID::Locator.locate(gid, only: Room)` → `locate_room(gid)`.
//!
//! `only:` is not a filter here, it is THE FINDER. globalid's own
//! `locate` constantizes the model name off the wire and finds on the
//! result; this pipeline will not emit a constant computed from a wire
//! string (and would not want to — a crafted name reaches any class),
//! so `runtime/global_id_locator.rb` narrows the API to "find on the
//! class the CALLER named" and checks the URI's name against it.
//!
//! That leaves a class OBJECT as the receiver of `.find` and `.name`.
//! CRuby dispatches through the singleton; a strict target has no
//! singleton to dispatch through, and spinel emits a call to a class
//! method `ActiveRecord::Base` never defines:
//!
//! ```text
//! call to undeclared function 'sp_ActiveRecord__Base_s_find'
//! ```
//!
//! Filed as matz/spinel#4217 for the compiler half (a class-value
//! dispatch that drops its class token), but the shape is ours to
//! avoid: `only:` is a LITERAL class at every call site in the corpus,
//! so there is nothing to dispatch on. Each site is rewritten to a
//! per-model entry point and `project::apply_global_id_locate` writes
//! that entry point with the finder spelled as a literal constant —
//! the same monomorphize-because-the-set-is-closed answer
//! [[project_class_side_new_binds_lexically]] took for a class-side
//! bare `new`.
//!
//! ONLY A LITERAL `only:` IS REWRITTEN. A computed one would need the
//! dispatch this pass exists to remove; it is left alone, so it still
//! reaches the generic `locate` — which runs correctly on the Ruby
//! lanes and is refused at compile time on a strict one. A refusal
//! naming the real construct beats a silent rewrite that finds on the
//! wrong class.
//!
//! Runs on BOTH Ruby-lane and strict-target emits, deliberately: the
//! overlay and the spinel binary share `runtime/global_id_locator.rb`,
//! and a rewrite that fired on only one lane would put a different
//! authorization path in each — the divergence class
//! `scripts/campfire-cable-walk` exists to catch.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use std::collections::BTreeSet;

/// The receiver spelling this pass claims. A single-segment `Locator`
/// is not accepted: `GlobalID::Locator` is the name the runtime module
/// is defined under, and nothing else should answer to a rewrite that
/// redirects a database read.
const RECEIVER: [&str; 2] = ["GlobalID", "Locator"];

pub fn apply_global_id_locate_lowering(app: &mut App) {
    let mut models: BTreeSet<Symbol> = BTreeSet::new();
    super::for_each_hook_body(app, &mut |expr| rewrite(expr, &mut models));
    for view in &mut app.views {
        rewrite(&mut view.body, &mut models);
    }
    app.global_id_locate_models.extend(models);
}

fn rewrite(expr: &mut Expr, models: &mut BTreeSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |child| rewrite(child, models));

    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "locate" || args.len() != 2 {
        return;
    }
    let ExprNode::Const { path } = &*recv.node else { return };
    if path.len() != RECEIVER.len()
        || path.iter().zip(RECEIVER).any(|(seg, want)| seg.as_str() != want)
    {
        return;
    }
    let Some(model) = only_kwarg_class(&args[1]) else { return };

    models.insert(model.clone());
    *method = Symbol::from(format!("locate_{}", entry_point_suffix(model.as_str())));
    args.truncate(1);
}

/// The class named by a lone trailing `only:` kwarg, or None for any
/// other shape — a second kwarg, a non-literal value, a positional
/// hash. `locate` takes exactly one keyword and it is required, so a
/// call carrying anything else is not the one this pass understands.
fn only_kwarg_class(arg: &Expr) -> Option<Symbol> {
    let ExprNode::Hash { entries, kwargs: true, .. } = &*arg.node else { return None };
    let [(key, value)] = &entries[..] else { return None };
    match &*key.node {
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "only" => {}
        _ => return None,
    }
    let ExprNode::Const { path } = &*value.node else { return None };
    Some(Symbol::from(
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
    ))
}

/// `Room` → `room`, `Rooms::Open` → `rooms__open`. A method name, so
/// the `::` separator `naming::underscore` writes as `/` becomes `__`
/// — the same flattening the strict targets already apply to a
/// namespaced class's own symbol.
pub fn entry_point_suffix(class_name: &str) -> String {
    crate::naming::underscore(class_name).replace('/', "__")
}
