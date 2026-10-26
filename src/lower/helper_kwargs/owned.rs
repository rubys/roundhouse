//! Calls a library class makes to its OWN flattened methods.
//!
//! The class-method and instance-method repairs in the parent module
//! find the callee from the receiver: a constant (`Builder.column(…)`)
//! or a value typed as an instance of a named class. Inside the class
//! itself the receiver names nothing:
//!
//! ```ruby
//! class Builder
//!   def field(name, type = nil, null: true, description: nil) = …
//!   def add(name) = field(name, null: false)        # receiverless
//!   def mark(name) = self.field(name, null: false)  # `self.`
//!   def self.report
//!     b = new                                       # typed `instance`
//!     b.field(:a, :string, null: false)
//!   end
//! end
//! ```
//!
//! A receiverless call and `self.` dispatch on the method's own side
//! (instance or class), and a value typed [`Ty::SelfInstance`] is an
//! instance of the class whose body it is in: the analyzer leaves a bare
//! `new` in a class method as `instance` because the class that runs
//! it is only known per copy (`class_body_new` monomorphizes inherited
//! class methods into each subclass, so the OWNER of the body is that
//! class). All three went unrepaired, and the keyword Hash bound to the
//! first flattened slot.
//!
//! The method is looked up the way Ruby looks it up for these
//! receivers: the owner, the modules it includes, then its superclass
//! chain, among the library classes.
//!
//! `self` is the object that RECEIVED the call, which can be an
//! instance of a subclass. When a subclass redefines the method with a
//! slot layout that moves the keywords differently, no one positional
//! spelling is right for every receiver, so the call is refused.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::MethodReceiver;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::{Slot, slot_of};

/// What a library class defines, by side, for the owner-relative lookup.
pub(super) struct OwnerIndex {
    parents: HashMap<String, String>,
    includes: HashMap<String, Vec<String>>,
    children: HashMap<String, Vec<String>>,
    /// Module → the classes and modules that include it. An includer's
    /// own definition comes before the module's in Ruby's lookup, so it
    /// is an override the module's body can reach through `self`.
    includers: HashMap<String, Vec<String>>,
    /// Every method a library class defines, flattened or not, with its
    /// slots when it is flattened (`None` when it cannot be respelled).
    defined: HashMap<(String, Side, Symbol), Option<Vec<Slot>>>,
    /// Classes and modules whose body `prepend`s a module. A prepended
    /// module sits ahead of the class itself in Ruby's lookup and is not
    /// one of `includes`, so a lookup through such a class is not resolved.
    prepending: HashSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Side {
    Instance,
    Class,
}

impl From<MethodReceiver> for Side {
    fn from(r: MethodReceiver) -> Self {
        match r {
            MethodReceiver::Instance => Side::Instance,
            MethodReceiver::Class => Side::Class,
        }
    }
}

impl OwnerIndex {
    pub(super) fn new(app: &App) -> Self {
        let names: HashSet<String> =
            app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
        let mut parents = HashMap::new();
        let mut includes: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut defined = HashMap::new();
        let mut prepending = HashSet::new();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str().to_string();
            if lc.unknown_calls.iter().any(|e| matches!(&*e.node,
                ExprNode::Send { recv, method, .. }
                    if method.as_str() == "prepend"
                        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))))
            {
                prepending.insert(name.clone());
            }
            if let Some(parent) = &lc.parent
                && let Some(parent) = resolve_const(&names, &name, parent.0.as_str())
            {
                children.entry(parent.clone()).or_default().push(name.clone());
                parents.insert(name.clone(), parent);
            }
            includes.insert(
                name.clone(),
                lc.includes
                    .iter()
                    .filter_map(|m| resolve_const(&names, &name, m.0.as_str()))
                    .collect(),
            );
            for m in &lc.methods {
                // Ruby uses the last definition.
                defined.insert(
                    (name.clone(), Side::from(m.receiver), m.name.clone()),
                    flattened_slots(m),
                );
            }
        }
        let mut includers: HashMap<String, Vec<String>> = HashMap::new();
        for (includer, modules) in &includes {
            for m in modules {
                includers.entry(m.clone()).or_default().push(includer.clone());
            }
        }
        Self { parents, includes, children, includers, defined, prepending }
    }

    /// `class`'s ancestors on `side`, in Ruby's method lookup order: the
    /// class, then each included module (last included first) followed
    /// by that module's own includes, recursively, then the superclass
    /// chain. Class-side lookup does not go through `include`. A name met
    /// twice is kept at every position, so `resolve` can see the
    /// ambiguity rather than guess which one Ruby's de-duplication keeps.
    fn ancestors(&self, class: &str, side: Side) -> Result<Vec<String>, ()> {
        fn with_includes(
            index: &OwnerIndex,
            module: &str,
            side: Side,
            path: &mut Vec<String>,
            out: &mut Vec<String>,
        ) -> Result<(), ()> {
            if path.iter().any(|p| p == module) {
                return Err(());
            }
            out.push(module.to_string());
            if side == Side::Instance {
                path.push(module.to_string());
                for m in index.includes.get(module).into_iter().flatten().rev() {
                    with_includes(index, m, side, path, out)?;
                }
                path.pop();
            }
            Ok(())
        }
        let mut out = Vec::new();
        let mut current = Some(class.to_string());
        let mut chain = HashSet::new();
        while let Some(c) = current {
            if !chain.insert(c.clone()) {
                return Err(());
            }
            with_includes(self, &c, side, &mut Vec::new(), &mut out)?;
            current = self.parents.get(&c).cloned();
        }
        Ok(out)
    }

    /// The definition a call on `side` of `class` reaches.
    fn resolve(&self, class: &str, side: Side, method: &Symbol) -> Resolved<'_> {
        let Ok(ancestors) = self.ancestors(class, side) else {
            return Resolved::Ambiguous;
        };
        let key = |c: &String| (c.clone(), side, method.clone());
        let first = ancestors.iter().position(|c| self.defined.contains_key(&key(c)));
        // A prepended module sits right ahead of the class that prepends
        // it (`Base` prepending `Loud`: `[Child, Loud, Base]`), so it can
        // take the call only from a class at or before the first definer;
        // one prepended further up is never reached. With no definer known,
        // any prepended module might define the name.
        if side == Side::Instance
            && ancestors[..first.map_or(ancestors.len(), |f| f + 1)]
                .iter()
                .any(|c| self.prepending.contains(c))
        {
            return Resolved::Ambiguous;
        }
        let Some(first) = first else {
            return Resolved::Missing;
        };
        // A module reachable at two positions in the chain: Ruby keeps
        // one (a class and its superclass both including `Helper` have it
        // once, at the superclass's place), and which one can decide the
        // binding when it defines the name. Dropping copies never moves
        // an entry that occurs once, so the first definer is the answer
        // when it occurs once, or when every definer is that same module.
        // Two different definers ordered by the kept copy stay ambiguous.
        let definers: Vec<&String> = ancestors.iter().filter(|c| self.defined.contains_key(&key(c))).collect();
        let first_name = &ancestors[first];
        let occurs_once = ancestors.iter().filter(|c| *c == first_name).count() == 1;
        if !occurs_once && definers.iter().any(|c| *c != first_name) {
            return Resolved::Ambiguous;
        }
        match self.defined.get(&key(&ancestors[first])) {
            Some(Some(slots)) => Resolved::Flattened(slots),
            _ => Resolved::Unflattened,
        }
    }

    /// The flattened slots a call on `side` of `class` reaches: `Ok(None)`
    /// for a definition that took no flattened keywords (or none known),
    /// `Err` when the lookup cannot be resolved statically.
    pub(super) fn lookup(&self, class: &str, side: Side, method: &Symbol) -> Result<Option<&Vec<Slot>>, ()> {
        match self.resolve(class, side, method) {
            Resolved::Flattened(slots) => Ok(Some(slots)),
            Resolved::Unflattened | Resolved::Missing => Ok(None),
            Resolved::Ambiguous => Err(()),
        }
    }

    /// Whether and how `class.side` defines `method`, distinguishing
    /// "does not define this at all" (any dispatch to it raises
    /// `NoMethodError` regardless of what a caller rewrites) from
    /// "defines it, but not with a flattened keyword ABI a caller can
    /// safely respell against" (a real keyword, `rest`, forwarding
    /// parameter, or no keywords at all — already binds correctly as
    /// written). Callers outside the owner-relative walk that need this
    /// distinction (a typed-receiver nilable union, `Gadget | Nil`) use
    /// this instead of `lookup`, which collapses both into `Ok(None)`.
    pub(super) fn defines(&self, class: &str, side: Side, method: &Symbol) -> Defined<'_> {
        match self.resolve(class, side, method) {
            Resolved::Flattened(slots) => Defined::Flattened(slots),
            Resolved::Unflattened => Defined::Unflattened,
            Resolved::Missing => Defined::Missing,
            Resolved::Ambiguous => Defined::Ambiguous,
        }
    }

    /// The definition `method` resolves to on every class or module that
    /// can be `self` in `class`'s body: its subclasses and its includers,
    /// and theirs, transitively, each resolved through its own full
    /// ancestry. `None` stands for one that cannot be respelled.
    fn overrides(&self, class: &str, side: Side, method: &Symbol) -> Vec<Option<&Vec<Slot>>> {
        let below = |c: &str| {
            let mut next = self.children.get(c).cloned().unwrap_or_default();
            if side == Side::Instance {
                next.extend(self.includers.get(c).cloned().unwrap_or_default());
            }
            next
        };
        let mut out = Vec::new();
        let mut stack: Vec<String> = below(class);
        let mut seen = HashSet::new();
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            match self.resolve(&c, side, method) {
                Resolved::Flattened(slots) => out.push(Some(slots)),
                Resolved::Unflattened | Resolved::Ambiguous => out.push(None),
                Resolved::Missing => {}
            }
            stack.extend(below(&c));
        }
        out
    }
}

enum Resolved<'a> {
    Flattened(&'a Vec<Slot>),
    Unflattened,
    Missing,
    Ambiguous,
}

/// `OwnerIndex::defines`'s result: see its doc comment. `Flattened`
/// carries the slots so a caller that only reaches this definition
/// through an ancestor or included module — not keyed directly in its
/// own `params.slots`-style map, which only ever holds each class's OWN
/// direct definitions — can still use its shape instead of wrongly
/// treating "defined, only inherited" as "cannot be resolved".
pub(super) enum Defined<'a> {
    Flattened(&'a Vec<Slot>),
    Unflattened,
    Missing,
    Ambiguous,
}

/// `name` as written inside `owner`, resolved against the library
/// classes the way a constant reference is: innermost namespace first.
fn resolve_const(names: &HashSet<String>, owner: &str, name: &str) -> Option<String> {
    let name = name.trim_start_matches("::");
    let mut scope: Vec<&str> = owner.split("::").collect();
    loop {
        let candidate = if scope.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", scope.join("::"))
        };
        if names.contains(&candidate) {
            return Some(candidate);
        }
        if scope.pop().is_none() {
            return None;
        }
    }
}

fn flattened_slots(m: &crate::dialect::MethodDef) -> Option<Vec<Slot>> {
    if m.params.iter().any(|p| p.rest || p.keyword || p.forwarding)
        || !m.params.iter().any(|p| p.from_keyword)
    {
        return None;
    }
    Some(m.params.iter().map(slot_of).collect())
}

impl OwnerIndex {
    /// Instance-side flattened slots by `(class, method)`, as the typed
    /// receiver lookup keys them.
    pub(super) fn instance_slots(&self) -> HashMap<(String, Symbol), Vec<Slot>> {
        self.defined
            .iter()
            .filter(|((_, side, _), _)| *side == Side::Instance)
            .filter_map(|((c, _, m), slots)| Some(((c.clone(), m.clone()), slots.clone()?)))
            .collect()
    }
}

/// The receiver side an owner-relative call dispatches on, or `None`
/// for a call this module does not resolve.
fn owner_side(recv: &Option<Expr>, side: Side) -> Option<Side> {
    match recv {
        None => Some(side),
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => Some(side),
        Some(r) if matches!(r.ty, Some(crate::ty::Ty::SelfInstance)) => Some(Side::Instance),
        Some(_) => None,
    }
}

/// The `**` half of [`super::refuse_flattened_keyword_splats`] for
/// owner-relative calls.
pub(super) fn refuse_library_class_splats(
    app: &mut App,
    index: &OwnerIndex,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    fn walk(
        e: &Expr,
        owner: &str,
        side: Side,
        index: &OwnerIndex,
        diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
    ) {
        e.node.for_each_child(&mut |c| walk(c, owner, side, index, diagnostics));
        let ExprNode::Send { recv, method, args, .. } = &*e.node else { return };
        let Some(side) = owner_side(recv, side) else { return };
        let refused = match index.lookup(owner, side, method) {
            Ok(Some(slots)) => super::splat_into_flattened(args, slots),
            Ok(None) => false,
            Err(()) => super::carries_splat(args),
        };
        if refused {
            super::refuse(e.span, super::SPLAT_REFUSAL, diagnostics);
        }
    }
    for lc in &app.library_classes {
        let owner = lc.name.0.as_str();
        for m in &lc.methods {
            walk(&m.body, owner, Side::from(m.receiver), index, diagnostics);
        }
    }
}

/// Repair, or refuse, every owner-relative call in the library classes'
/// method bodies.
pub(super) fn rewrite_library_classes(
    app: &mut App,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    let index = OwnerIndex::new(app);
    if !index.defined.values().any(Option::is_some) {
        return;
    }
    for lc in &mut app.library_classes {
        let owner = lc.name.0.as_str().to_string();
        for m in &mut lc.methods {
            let side = Side::from(m.receiver);
            rewrite(&mut m.body, &owner, side, &index, diagnostics);
        }
    }
}

const SEND_METHODS: &[&str] = &["send", "public_send", "__send__"];

const AMBIGUOUS: &str = "the method this call reaches cannot be resolved statically (an include cycle, a prepended module, or two modules whose order depends on which copy of one Ruby keeps), so its keyword slots are not known";

fn rewrite(
    e: &mut Expr,
    owner: &str,
    side: Side,
    index: &OwnerIndex,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, owner, side, index, diagnostics));
    rewrite_node(e, owner, side, index, diagnostics);
}

fn rewrite_node(
    e: &mut Expr,
    owner: &str,
    side: Side,
    index: &OwnerIndex,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    let span = e.span;
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else {
        return;
    };
    let Some(side) = owner_side(recv, side) else { return };
    if SEND_METHODS.contains(&method.as_str()) {
        refuse_send(span, args, |name| !matches!(index.lookup(owner, side, name), Ok(None)), diagnostics);
        return;
    }
    if !super::carries_keywords(args) {
        return;
    }
    let slots = match index.lookup(owner, side, method) {
        Ok(Some(slots)) => slots,
        Ok(None) => return,
        Err(()) => {
            super::refuse(span, AMBIGUOUS, diagnostics);
            return;
        }
    };
    let mut respelled = args.clone();
    if let Err(reason) = super::respell(&mut respelled, slots, true) {
        super::refuse(span, reason, diagnostics);
        return;
    }
    for other in index.overrides(owner, side, method) {
        let mut theirs = args.clone();
        let agrees = other.is_some_and(|slots| super::respell(&mut theirs, slots, true).is_ok())
            && theirs == respelled;
        if !agrees {
            super::refuse(
                span,
                "a subclass or includer redefines this method with different keyword slots, so no one positional order binds for every receiver",
                diagnostics,
            );
            return;
        }
    }
    *args = respelled;
}

/// `recv.send(:name, …, k: v)` to a method whose keywords were
/// flattened: the dispatch is dynamic, so the call is not moved; it is
/// refused when the name is a literal naming such a method, which is
/// the one case where it is known to bind wrongly.
pub(super) fn refuse_send(
    span: crate::span::Span,
    args: &[Expr],
    flattened: impl Fn(&Symbol) -> bool,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    let Some(first) = args.first() else { return };
    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*first.node else {
        return;
    };
    if !super::carries_keywords(&args[1..]) {
        return;
    }
    if flattened(name) {
        super::refuse(
            span,
            "keywords passed through `send` to a method whose optional keywords are positional slots",
            diagnostics,
        );
    }
}
