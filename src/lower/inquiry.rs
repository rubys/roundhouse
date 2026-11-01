//! ActiveSupport's `String#inquiry` → equality against the label.
//!
//! `"bot_key".inquiry` answers a `StringInquirer`, a String subclass
//! whose `method_missing` turns any predicate into a comparison:
//! `authenticated_by.bot_key?` is `authenticated_by == "bot_key"`. Both
//! halves are unreachable for a transpiled runtime — no target can
//! reopen the builtin, and a `method_missing` predicate is exactly the
//! dynamic dispatch the runtime is required not to have.
//!
//! It is also unnecessary. The predicate name IS the comparand, known
//! at compile time at every call site, so the whole thing folds:
//!
//! ```text
//! authenticated_by.bot_key?                 =>  authenticated_by == "bot_key"
//! involvement_previously_was.inquiry.invisible?
//!                                           =>  involvement_previously_was == "invisible"
//! ```
//!
//! Two rewrites, bottom-up in one walk:
//!
//!   * `<recv>.inquiry` → `<recv>`. The inquirer IS its string; the
//!     subclass exists only to host the predicates this pass removes.
//!   * `<recv>.<name>?` → `<recv> == "<name>"`, when the receiver types
//!     as a String and `<name>?` is not a method a String actually has.
//!
//! That second condition is what keeps the pass honest. `empty?`,
//! `present?`, `start_with?` and their siblings are real String methods
//! and are left alone — asked of `analyze::string_answers`, which is
//! the body-typer's own catalog and the only thing that knows the
//! ActiveSupport core_ext predicates are there. Asking the CLASS
//! registry instead, as this pass first did, answers "String has no
//! methods at all": every predicate looked unknown and
//! `notice.present?` became `notice == "present"`, wrong output that
//! compare never sees because a page without a flash renders the same
//! either way. What remains — a predicate no String answers — has
//! exactly one meaning in Ruby, and it is this one: on a plain String
//! it is a NoMethodError, and on the inquirer it is the comparison.
//!
//! A predicate the app itself defines on `String` is also a real String
//! method: campfire's `lib/rails_ext/string.rb` adds `all_emoji?`, and
//! the inquirer, a String subclass, answers it the same way.
//!
//! The by-name evidence is only as good as the call site's reach. A
//! method name collected from one class (`ApiClient#kind` ends in
//! `StringInquirer.new`) says nothing about a receiver-less `kind` in
//! an unrelated class or mixin, where it resolves to that class's own
//! `kind` (a GraphQL type's `TypeKind`, whose `interface?` is real).
//! A receiver-less call therefore folds only when the method dispatch
//! selects for its owner (the nearest definer along its include /
//! superclass chain, by full class name) answers the inquirer, and an
//! explicit receiver only when its typed (or written) class's does.
//!
//! An app defining its own `inquiry` disables the pass wholesale, the
//! same coarse opt-out `exclude_predicate` takes beside this file: the
//! name would then mean something the app chose, and a receiver type
//! rarely names the class that defined it.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_inquiry_lowering(app: &mut App) {
    if app_defines_inquiry(app) {
        return;
    }
    let facts = Facts {
        inquirers: inquirer_methods(app),
        app_string_methods: app_string_methods(app),
        reach: Reach::collect(app),
    };
    super::for_each_owned_hook_body(app, &mut |owner, body| rewrite(body, &facts, owner));
    for view in &mut app.views {
        rewrite(&mut view.body, &facts, None);
    }
}

/// The inquirer-returning methods and the tail test now live in
/// `crate::analyze::inquiry`, shared with the body typer so the
/// analyze-only drivers (`check`, LSP, MCP) answer the predicate the
/// same way this pass folds it.
fn inquirer_methods(app: &App) -> crate::det::HashSet<Symbol> {
    crate::analyze::inquiry::inquirer_methods(app)
}

struct Facts {
    inquirers: crate::det::HashSet<Symbol>,
    app_string_methods: crate::det::HashSet<Symbol>,
    reach: Reach,
}

/// Each app class's instance methods by name (whether the method answers
/// an inquirer), and its ancestors in Ruby's lookup order (later includes
/// first, then the superclass), so a call can be checked against the
/// method dispatch actually selects for its owner. Keyed by the full class
/// name; a written include or superclass resolves through its lexical
/// namespace first, as Ruby's constant lookup does.
#[derive(Default)]
struct Reach {
    defined: crate::det::HashMap<String, crate::det::HashMap<Symbol, bool>>,
    ancestors: crate::det::HashMap<String, Vec<String>>,
}

impl Reach {
    fn collect(app: &App) -> Reach {
        let mut r = Reach::default();
        let mut known: crate::det::HashSet<String> = crate::det::HashSet::default();
        known.extend(app.models.iter().map(|m| m.name.0.as_str().to_string()));
        known.extend(app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()));
        known.extend(app.controllers.iter().map(|c| c.name.0.as_str().to_string()));
        let resolve = |owner: &str, written: &crate::ident::ClassId| -> String {
            let name = written.0.as_str().trim_start_matches("::");
            if written.0.as_str().starts_with("::") {
                return name.to_string();
            }
            let mut scope: Vec<&str> = owner.split("::").collect();
            scope.pop();
            while !scope.is_empty() {
                let candidate = format!("{}::{name}", scope.join("::"));
                if known.contains(&candidate) {
                    return candidate;
                }
                scope.pop();
            }
            name.to_string()
        };
        let define = |r: &mut Reach, owner: &str, method: &Symbol, inquiry: bool| {
            let slot = r.defined.entry(owner.to_string()).or_default().entry(method.clone()).or_insert(false);
            *slot |= inquiry;
        };
        for model in &app.models {
            let owner = model.name.0.as_str();
            for m in model.methods().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
                define(&mut r, owner, &m.name, crate::analyze::inquiry::tail_is_inquiry(&m.body));
            }
            let mut up: Vec<String> = crate::analyze::model_includes(model).iter().rev().map(|i| resolve(owner, i)).collect();
            up.extend(model.parent.iter().map(|p| resolve(owner, p)));
            r.ancestors.entry(owner.to_string()).or_default().extend(up);
        }
        for lc in &app.library_classes {
            let owner = lc.name.0.as_str();
            for m in lc.methods.iter().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
                define(&mut r, owner, &m.name, crate::analyze::inquiry::tail_is_inquiry(&m.body));
            }
            let mut up: Vec<String> = lc.includes.iter().rev().map(|i| resolve(owner, i)).collect();
            up.extend(lc.parent.iter().map(|p| resolve(owner, p)));
            r.ancestors.entry(owner.to_string()).or_default().extend(up);
        }
        for c in &app.controllers {
            let owner = c.name.0.as_str();
            for item in &c.body {
                if let crate::dialect::ControllerBodyItem::Action { action, .. } = item {
                    define(&mut r, owner, &action.name, crate::analyze::inquiry::tail_is_inquiry(&action.body));
                }
            }
            if let Some(p) = &c.parent {
                let parent = resolve(owner, p);
                r.ancestors.entry(owner.to_string()).or_default().push(parent);
            }
        }
        r
    }

    /// Does the `method` that dispatch selects for an instance of `owner`
    /// (the nearest definer in its lookup order) answer an inquirer? A
    /// nearer override that answers anything else decides it, as Ruby's
    /// lookup does.
    fn owner_reaches(&self, owner: &crate::ident::ClassId, method: &Symbol) -> bool {
        let mut order = vec![owner.0.as_str().trim_start_matches("::").to_string()];
        let mut seen = crate::det::HashSet::default();
        let mut i = 0;
        while i < order.len() {
            let cur = order[i].clone();
            i += 1;
            if !seen.insert(cur.clone()) {
                continue;
            }
            if let Some(inquiry) = self.defined.get(&cur).and_then(|methods| methods.get(method)) {
                return *inquiry;
            }
            if let Some(up) = self.ancestors.get(&cur) {
                // Depth-first in lookup order: this class's ancestors come
                // before the next sibling's.
                for (k, next) in up.iter().enumerate() {
                    order.insert(i + k, next.clone());
                }
            }
        }
        false
    }
}

/// The methods the app defines by reopening `String`, its included
/// modules' among them.
fn app_string_methods(app: &App) -> crate::det::HashSet<Symbol> {
    let mut out = crate::det::HashSet::default();
    let mut seen = crate::det::HashSet::default();
    let mut stack = vec![crate::ident::ClassId(Symbol::from("String"))];
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        for lc in app.library_classes.iter().filter(|lc| lc.name == id) {
            out.extend(
                lc.methods
                    .iter()
                    .filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance)
                    .map(|m| m.name.clone()),
            );
            stack.extend(lc.includes.iter().cloned());
        }
    }
    out
}

fn rewrite(expr: &mut Expr, facts: &Facts, owner: Option<&crate::ident::ClassId>) {
    let inquirers = &facts.inquirers;
    // `<recv>.inquiry.<name>?` — the DIRECT pair, matched BEFORE the
    // recursive walk folds the `.inquiry` away.
    //
    // The `Ty::Str` gate below cannot see this one: campfire writes
    // `involvement_previously_was.inquiry.invisible?`, and the receiver
    // is a synthesized Dirty reader whose value comes out of an untyped
    // saved-changes diff. The `inquirers` set does not cover it either —
    // that set is for a METHOD whose body ends in `.inquiry`, and here
    // the call is at the SITE.
    //
    // Which makes this the strongest evidence of the three and the only
    // one the pass was throwing away: an explicit `.inquiry` in the
    // source says the author meant the inquirer, whatever the receiver
    // types as. Same reason the doc above gives for collecting
    // `inquirers` first — the rewrite destroys the evidence — applied
    // one level in.
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node {
        if args.is_empty() {
            if let Some(label) = method.as_str().strip_suffix('?') {
                if !label.is_empty() && !facts.app_string_methods.contains(method) {
                    if let Some(inner) = crate::analyze::inquiry::inquirer_value(recv) {
                        let mut inner = inner.clone();
                        rewrite(&mut inner, facts, owner);
                        let span = expr.span;
                        *expr = eq_label(span, inner, label);
                        return;
                    }
                }
            }
        }
    }

    expr.node.for_each_child_mut(&mut |c| rewrite(c, facts, owner));

    // `<recv>.inquiry` / `ActiveSupport::StringInquirer.new(<recv>)` — the inquirer is its string.
    if let Some(inner) = crate::analyze::inquiry::inquirer_value(expr) {
        *expr = inner.clone();
        return;
    }
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if !args.is_empty() {
        return;
    }

    // `<recv>.<name>?` on a String the registry doesn't answer that for.
    let Some(label) = method.as_str().strip_suffix('?') else { return };
    if label.is_empty()
        || crate::analyze::string_answers(method)
        || facts.app_string_methods.contains(method)
    {
        return;
    }
    // Either the analyzer typed the receiver a String, or the receiver
    // is a call to a method whose value came from `.inquiry` — the fact
    // collected above, since the fold erases the call itself.
    let recv_is_inquirer = matches!(
        &*recv.node,
        ExprNode::Send { recv: call_recv, method, .. } if inquirers.contains(method)
            && inquirer_call_reaches(facts, owner, call_recv.as_ref(), method)
    );
    if !matches!(recv.ty, Some(Ty::Str)) && !recv_is_inquirer {
        return;
    }
    let recv = recv.clone();
    let span = expr.span;
    *expr = eq_label(span, recv, label);
}

/// Can the call `<call_recv>.<method>` reach an inquirer-returning
/// definer? A receiver-less call (or `self.`) dispatches on the owner's
/// own `self`; an explicit receiver is judged by the class it is typed
/// or written as. A receiver of unknown class stays unfolded: the name
/// alone does not say which class's `<method>` it is, and an unfolded
/// predicate on a real inquirer fails loudly where a wrong fold
/// compares a non-String silently.
fn inquirer_call_reaches(
    facts: &Facts,
    owner: Option<&crate::ident::ClassId>,
    call_recv: Option<&Expr>,
    method: &Symbol,
) -> bool {
    let class_of = |recv: &Expr| -> Option<crate::ident::ClassId> {
        match (&*recv.node, &recv.ty) {
            (ExprNode::SelfRef, _) => owner.cloned(),
            (ExprNode::Const { path }, _) => Some(crate::ident::ClassId(Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str(),
            ))),
            (_, Some(Ty::Class { id, .. })) => Some(id.clone()),
            _ => None,
        }
    };
    match call_recv {
        // Views and seeds belong to no class; keep the name-keyed fact.
        None => owner.map_or(true, |o| facts.reach.owner_reaches(o, method)),
        Some(recv) => class_of(recv).is_some_and(|c| facts.reach.owner_reaches(&c, method)),
    }
}

/// `<recv> == "<label>"`, stamped Bool. The one shape both the direct
/// `.inquiry.<label>?` pair and the typed-receiver predicate fold to,
/// so the two cannot drift.
fn eq_label(span: crate::span::Span, recv: Expr, label: &str) -> Expr {
    let mut out = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("=="),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Str { value: label.to_string() } },
            )],
            block: None,
            parenthesized: false,
        },
    );
    out.ty = Some(Ty::Bool);
    out
}

/// True when any app class defines `inquiry` itself.
fn app_defines_inquiry(app: &App) -> bool {
    let named = |n: &Symbol| n.as_str() == "inquiry";
    app.models.iter().any(|m| m.methods().any(|d| named(&d.name)))
        || app
            .library_classes
            .iter()
            .any(|lc| lc.methods.iter().any(|d| named(&d.name)))
        || app
            .controllers
            .iter()
            .any(|c| c.actions().any(|a| named(&a.name)))
}
