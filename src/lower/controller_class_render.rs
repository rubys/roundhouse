//! `ApplicationController.render partial: "users/mention", locals: {
//! user: user }` → `Views::Users.mention(user)`.
//!
//! Rails' class-side renderer: a controller can render a template with
//! no request, which is how a TEST builds a fragment to compare against
//! (campfire's `mention_attachment_for` embeds the rendered mention
//! inside an `<action-text-attachment>`). It is the same partial the
//! views render, reached from outside a view.
//!
//! Bound through `view_to_library::partial_call_contracts` — the DEF
//! SITE's own contract, the same one a `render partial:` call site in a
//! view binds against — so the two cannot disagree about what the
//! partial takes.
//!
//! **Declines whenever the partial needs more than its record.** A
//! partial with closure ivars or extra locals is written to be rendered
//! from a view that has them; a class-side call has no such context, so
//! there is nothing to bind and guessing would pass the wrong
//! arguments. Rails would raise there too, one step later.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_controller_class_render(app: &mut App) {
    let contracts = crate::lower::view_to_library::partial_call_contracts(
        &app.views,
        &app.controllers,
        &app.library_classes,
    );
    if contracts.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &contracts));
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &contracts);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &contracts);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &contracts);
        }
    }
}

type Contracts = std::collections::HashMap<
    (String, String),
    crate::lower::view_to_library::PartialCallContract,
>;

fn rewrite(expr: &mut Expr, contracts: &Contracts) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, contracts));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "render" || args.len() != 1 {
        return;
    }
    // A CONTROLLER constant — Rails' class-side renderer lives there.
    let ExprNode::Const { path } = &*r.node else { return };
    if !path.last().is_some_and(|s| s.as_str().ends_with("Controller")) {
        return;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return };
    let opt = |name: &str| {
        entries.iter().find_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => Some(v),
            _ => None,
        })
    };
    // Only `partial:` + `locals:`. Any other option is one this does
    // not read, and dropping it silently is how a render turns into
    // the wrong markup.
    if !entries.iter().all(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if matches!(value.as_str(), "partial" | "locals"))
    }) {
        return;
    }
    let Some(ExprNode::Lit { value: Literal::Str { value: partial } }) =
        opt("partial").map(|p| &*p.node)
    else {
        return;
    };
    let Some((dir, stem)) = partial.rsplit_once('/') else { return };
    let module = crate::naming::camelize_path(&crate::naming::snake_case(dir));
    let Some(contract) = contracts.get(&(module.clone(), stem.to_string())) else { return };
    // The record is all a class-side call can bind — except the FLASH
    // PAIR, which every view method carries as a defaulted tail
    // (`def self.mention(user, notice = nil, alert = nil)`) and which a
    // caller outside a request has no business supplying anyway.
    // Anything else in the contract is a value the partial's body
    // genuinely reads and a class-side call has nowhere to get, so
    // declining is the honest answer — Rails raises there too, one step
    // later.
    if !contract.closure.is_empty()
        || contract.extras.iter().any(|e| e != "notice" && e != "alert")
    {
        return;
    }
    let locals = match opt("locals").map(|l| &*l.node) {
        Some(ExprNode::Hash { entries, .. }) => entries.clone(),
        Some(_) => return,
        None => Vec::new(),
    };
    let record = locals.iter().find_map(|(k, v)| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == contract.record => {
            Some(v.clone())
        }
        _ => None,
    });
    // Exactly the record, nothing else — an unbound extra local is a
    // value the partial would never see.
    if locals.len() != record.iter().len() {
        return;
    }
    let span = expr.span;
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: std::iter::once(Symbol::from("Views"))
                        .chain(module.split("::").map(Symbol::from))
                        .collect(),
                },
            )),
            method: Symbol::from(stem),
            args: record.into_iter().collect(),
            block: None,
            parenthesized: true,
        },
    );
}
