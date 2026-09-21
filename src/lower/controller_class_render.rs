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
//!
//! ## The attachment's own partial
//!
//! `ApplicationController.render partial: attachment.to_partial_path,
//! locals: { opengraph_embed: attachment }` — the partial is not a
//! literal but the ATTACHMENT'S, which is per node: whichever attachable
//! the node resolves to names it. That is exactly the dispatch
//! `Content.render_attachment` is generated to make
//! (`project::apply_content_layout`: one arm per attachable class, the
//! partial's local bound to the attachable), so the call becomes
//! `ActionText::Content.render_attachment(attachment)` when the one
//! local is that same attachment. The local's NAME is not checked: the
//! generated arm binds the partial's declared local, and a working app
//! (this one renders under Rails) can only have named it that.
//!
//! In a TEST class the local carries no type yet — test bodies are typed
//! when they are lowered, after this pass — so the attachment is known
//! syntactically instead: a local assigned from the class's own helper
//! whose tail expression is `ActionText::Attachment.from_node(…)`
//! (campfire's `attachment = attachment_for(…)`). A receiver typed
//! `ActionText::Attachment` counts wherever the type is there.

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
    let none = std::collections::HashSet::new();
    super::for_each_hook_body(app, &mut |e| rewrite(e, &contracts, &none));
    for tm in &mut app.test_modules {
        let builders: std::collections::HashSet<Symbol> = tm
            .helpers
            .iter()
            .filter(|h| builds_attachment(&h.body))
            .map(|h| h.name.clone())
            .collect();
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &contracts, &attachment_locals(setup, &builders));
        }
        for t in &mut tm.tests {
            let locals = attachment_locals(&t.body, &builders);
            rewrite(&mut t.body, &contracts, &locals);
        }
        for m in &mut tm.helpers {
            let locals = attachment_locals(&m.body, &builders);
            rewrite(&mut m.body, &contracts, &locals);
        }
    }
}

/// Does this body end in `ActionText::Attachment.from_node(…)` — the
/// constructor, so whatever calls the method holds an Attachment?
fn builds_attachment(body: &Expr) -> bool {
    let tail = match &*body.node {
        ExprNode::Seq { exprs } => match exprs.last() {
            Some(e) => e,
            None => return false,
        },
        _ => body,
    };
    let ExprNode::Send { recv: Some(recv), method, .. } = &*tail.node else { return false };
    if method.as_str() != "from_node" {
        return false;
    }
    let ExprNode::Const { path } = &*recv.node else { return false };
    let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    joined == "ActionText::Attachment"
}

/// Locals in `body` assigned from a receiverless call to one of the
/// attachment `builders`.
fn attachment_locals(
    body: &Expr,
    builders: &std::collections::HashSet<Symbol>,
) -> std::collections::HashSet<Symbol> {
    let mut out = std::collections::HashSet::new();
    if builders.is_empty() {
        return out;
    }
    fn walk(e: &Expr, builders: &std::collections::HashSet<Symbol>, out: &mut std::collections::HashSet<Symbol>) {
        if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } = &*e.node {
            if let ExprNode::Send { recv: None, method, .. } = &*value.node {
                if builders.contains(method) {
                    out.insert(name.clone());
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, builders, out));
    }
    walk(body, builders, &mut out);
    out
}

type Contracts = std::collections::HashMap<
    (String, String),
    crate::lower::view_to_library::PartialCallContract,
>;

fn rewrite(expr: &mut Expr, contracts: &Contracts, attachment_locals: &std::collections::HashSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, contracts, attachment_locals));
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
    if let Some(attachment) = attachments_own_partial(opt("partial"), opt("locals"), attachment_locals) {
        let span = expr.span;
        *expr = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Const {
                        path: vec![Symbol::from("ActionText"), Symbol::from("Content")],
                    },
                )),
                method: Symbol::from("render_attachment"),
                args: vec![attachment],
                block: None,
                parenthesized: true,
            },
        );
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

/// `partial: a.to_partial_path, locals: { k: a }` with `a` an
/// `ActionText::Attachment`: the attachment, when the call is that
/// shape and nothing else.
fn attachments_own_partial(
    partial: Option<&Expr>,
    locals: Option<&Expr>,
    attachment_locals: &std::collections::HashSet<Symbol>,
) -> Option<Expr> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*partial?.node
    else {
        return None;
    };
    if method.as_str() != "to_partial_path" || !args.is_empty() {
        return None;
    }
    let typed_attachment = matches!(
        recv.ty.as_ref(),
        Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "ActionText::Attachment"
    );
    let is_attachment = match &*recv.node {
        ExprNode::Var { name, .. } => typed_attachment || attachment_locals.contains(name),
        ExprNode::Ivar { .. } => typed_attachment,
        _ => false,
    };
    if !is_attachment {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*locals?.node else { return None };
    let [(_, value)] = entries.as_slice() else { return None };
    if value != recv {
        return None;
    }
    Some(recv.clone())
}
