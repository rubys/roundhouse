//! `on_subscribe :present, unless: :subscription_rejected?` → a real
//! `after_subscribe` method on the channel.
//!
//! Action Cable's `Channel::Base` includes `ActiveSupport::Callbacks` and
//! declares `after_subscribe` (aliased `on_subscribe`) and
//! `after_unsubscribe` (aliased `on_unsubscribe`). The declaration is a
//! class-body macro whose effect is a callback chain the framework runs
//! around `subscribed`/`unsubscribed`; nothing about that survives into a
//! tree with no `ActiveSupport::Callbacks`, so the call landed in
//! `unknown_calls` and was dropped with a `lower_residue` warning.
//!
//! It is not a nicety. campfire's `PresenceChannel` does ALL of its work
//! in those two hooks — `on_subscribe :present` is the `memberships`
//! UPDATE that records a user as being in a room, and
//! `on_unsubscribe :absent` is the one that takes them out again. A
//! channel that ran `subscribed` and neither hook subscribed correctly
//! and wrote nothing, which is invisible in a functional walk (every
//! frame still arrives) and is precisely the cost a connect-storm
//! benchmark exists to measure. The cable sweep found it by measuring
//! the two lanes against each other: Rails wrote a row per socket and
//! the binary wrote none.
//!
//! ## What it becomes
//!
//! One generated method per hook, guards inlined, callbacks in
//! declaration order:
//!
//! ```ruby
//! def after_subscribe
//!   if !subscription_rejected?
//!     present
//!   end
//!   nil
//! end
//! ```
//!
//! A method and not a table, for the reason every lowering here prefers
//! one: the strict targets resolve calls statically, so a chain walked at
//! run time would be a list of Symbols nothing can dispatch. The runtime
//! base declares both names as no-ops and the dispatcher calls them
//! unconditionally — the same shape `subscribed` already has.
//!
//! ## Inheritance is INLINED, not `super`
//!
//! Rails' callback chains inherit: a hook declared on a parent channel
//! runs for the child too, parent first. Rather than emit `super` — which
//! would need the parent to have a generated method and would put a
//! virtual call on the strict targets' path — an ancestor's callbacks are
//! copied into the child's method ahead of its own. Same order, same
//! effect, one resolvable body.

use crate::dialect::{LibraryClass, MethodDef};
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;

const ROOT: &str = "ActionCable::Channel::Base";

/// One declared callback: the method to run, and the guard that decides.
#[derive(Clone)]
struct Callback {
    hook: Hook,
    method: Symbol,
    guard: Option<Guard>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hook {
    Subscribe,
    Unsubscribe,
}

impl Hook {
    fn generated_name(self) -> &'static str {
        match self {
            Hook::Subscribe => "after_subscribe",
            Hook::Unsubscribe => "after_unsubscribe",
        }
    }
}

/// `if: :ready?` / `unless: :subscription_rejected?`. Only the Symbol
/// form is modelled; a lambda guard would need a block lowered into the
/// generated body and no corpus channel writes one.
#[derive(Clone)]
struct Guard {
    method: Symbol,
    negated: bool,
}

/// Turn every channel's subscribe/unsubscribe callback declarations into
/// generated methods. Runs over `library_classes` in place.
pub fn lower_channel_callbacks(app: &mut crate::App) {
    let channels = channel_class_names(app);
    if channels.is_empty() {
        return;
    }

    // Declared callbacks per class, consumed out of `unknown_calls` so
    // the emit's dropped-call ledger stops reporting them.
    let mut declared: Vec<(String, Vec<Callback>)> = Vec::new();
    for lc in app.library_classes.iter_mut() {
        let name = lc.name.0.as_str().to_string();
        if !channels.contains(&name) {
            continue;
        }
        let found = take_callback_decls(lc);
        if !found.is_empty() {
            declared.push((name, found));
        }
    }
    if declared.is_empty() {
        return;
    }

    // Ancestors first (Rails runs the parent's chain before the
    // child's), then the class's own.
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    for (i, lc) in app.library_classes.iter().enumerate() {
        let name = lc.name.0.as_str();
        if !channels.contains(&name.to_string()) {
            continue;
        }
        let chain = inherited_chain(app, name, &declared);
        if chain.is_empty() {
            continue;
        }
        let src = synthesized_source(name, &chain);
        let methods = match crate::ingest::ingest_library_classes(src.as_bytes(), "<channel>") {
            Ok(classes) => classes.into_iter().flat_map(|c| c.methods).collect(),
            Err(err) => {
                super::survey::record(&err);
                Vec::new()
            }
        };
        generated.push((i, methods));
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
}

/// Every app class descending from `ActionCable::Channel::Base`, by
/// transitive descent. Same fixpoint `project::apply_cable_channels`
/// walks, and for the same reason: `library_classes` is in ingest order,
/// so a subclass can be listed before the parent that puts it in the set.
fn channel_class_names(app: &crate::App) -> Vec<String> {
    let mut known: Vec<String> = vec![ROOT.to_string()];
    let mut channels: Vec<String> = Vec::new();
    loop {
        let before = known.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str().to_string();
            if lc.is_module || known.contains(&name) {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            if known.contains(&parent.0.as_str().to_string()) {
                known.push(name.clone());
                channels.push(name);
            }
        }
        if known.len() == before {
            break;
        }
    }
    channels
}

/// The callbacks that run for `name`: every ancestor's, outermost first,
/// then its own. `declared` holds only classes that declared any.
fn inherited_chain(
    app: &crate::App,
    name: &str,
    declared: &[(String, Vec<Callback>)],
) -> Vec<Callback> {
    let mut lineage: Vec<&str> = vec![name];
    let mut cursor = name;
    // Bounded by the class count: a cycle in `parent` would be an
    // ingest bug, and spinning here would hang the compile.
    for _ in 0..app.library_classes.len() {
        let Some(lc) = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == cursor)
        else {
            break;
        };
        let Some(parent) = &lc.parent else { break };
        let parent = parent.0.as_str();
        if lineage.contains(&parent) {
            break;
        }
        lineage.push(parent);
        cursor = parent;
    }
    lineage
        .iter()
        .rev()
        .flat_map(|c| {
            declared
                .iter()
                .find(|(n, _)| n == c)
                .map(|(_, cbs)| cbs.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// Consume `on_subscribe` / `after_subscribe` / `on_unsubscribe` /
/// `after_unsubscribe` out of a channel's class body.
///
/// A declaration whose argument is not a plain Symbol (a block form) is
/// LEFT in `unknown_calls`, so it keeps its `lower_residue` warning
/// rather than being silently half-modelled.
fn take_callback_decls(lc: &mut LibraryClass) -> Vec<Callback> {
    let mut out = Vec::new();
    lc.unknown_calls.retain(|call| {
        let ExprNode::Send { recv: None, method, args, block, .. } = &*call.node else {
            return true;
        };
        let hook = match method.as_str() {
            "on_subscribe" | "after_subscribe" => Hook::Subscribe,
            "on_unsubscribe" | "after_unsubscribe" => Hook::Unsubscribe,
            _ => return true,
        };
        if block.is_some() {
            return true;
        }
        let mut names: Vec<Symbol> = Vec::new();
        let mut guard: Option<Guard> = None;
        let mut understood = true;
        for a in args {
            match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => names.push(value.clone()),
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                            understood = false;
                            continue;
                        };
                        let negated = match key.as_str() {
                            "if" => false,
                            "unless" => true,
                            _ => {
                                understood = false;
                                continue;
                            }
                        };
                        match &*v.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => {
                                guard = Some(Guard { method: value.clone(), negated });
                            }
                            _ => understood = false,
                        }
                    }
                }
                _ => understood = false,
            }
        }
        if !understood || names.is_empty() {
            return true;
        }
        for method in names {
            out.push(Callback { hook, method, guard: guard.clone() });
        }
        false
    });
    out
}

/// The generated class body: one method per hook that has callbacks.
fn synthesized_source(class: &str, chain: &[Callback]) -> String {
    let mut body = String::new();
    for hook in [Hook::Subscribe, Hook::Unsubscribe] {
        let mine: Vec<&Callback> = chain.iter().filter(|c| c.hook == hook).collect();
        if mine.is_empty() {
            continue;
        }
        body.push_str(&format!("  def {}\n", hook.generated_name()));
        for cb in mine {
            match &cb.guard {
                // `!guard` rather than `unless`: the emitted Ruby is read
                // by twelve other emitters, and a negated `if` is the one
                // form every one of them already lowers.
                Some(g) => body.push_str(&format!(
                    "    if {}{}\n      {}\n    end\n",
                    if g.negated { "!" } else { "" },
                    g.method.as_str(),
                    cb.method.as_str()
                )),
                None => body.push_str(&format!("    {}\n", cb.method.as_str())),
            }
        }
        // An explicit nil: the last callback's value is the method's
        // otherwise, and the dispatcher's slot would take its type.
        body.push_str("    nil\n  end\n\n");
    }
    format!("class {class}\n{body}end\n")
}
