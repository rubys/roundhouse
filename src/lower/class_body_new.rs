//! A bare `new` in a CLASS BODY is the class's own constructor.
//!
//! `self` in a class body — and in a `def self.` method — is the class,
//! so Ruby reads `new(name: "bell")` there as `Sound.new(name: "bell")`.
//! campfire's `Sound::BUILTIN` is fifty-six of them in one array
//! literal, and the emit replayed the bare spelling:
//!
//! ```text
//! app/models/sound.rb:39: unsupported call:
//!   node 95214 (CallNode `new`) recv=-/ty-1 argc=1 arg0ty28
//! ```
//!
//! A receiverless call has no receiver to resolve against, and `new`
//! is not a free function anywhere. The same array with the receiver
//! spelled out compiles and runs on spinel today (probed), which is
//! what makes this ours rather than a compiler gap.
//!
//! Scoped to the two positions where implicit self IS the class:
//! class-body constant initializers (they run at load time, in the
//! class body) and class-side method bodies. An INSTANCE method's bare
//! `new` is a NoMethodError in Ruby too, so there is nothing there to
//! preserve.
//!
//! AND `self` IS NOT ALWAYS THE OWNER. In a class-side method that a
//! subclass INHERITS, `self` at call time is the subclass — that is the
//! whole point of the class-side template method:
//!
//! ```ruby
//! class ActionText::Content::Filter          # campfire, rails_ext/filter.rb
//!   class << self
//!     def apply(content)
//!       filter = new(content)                # `self` is the SUBCLASS
//!       filter.applicable? ? ... : content
//!     end
//!   end
//!   def applicable? = raise NotImplementedError
//! end
//! ```
//!
//! Binding that to the owner made every `ContentFilters::*.apply` build
//! the ABSTRACT BASE, whose `applicable?` raises — and campfire wraps
//! the filter chain in its own `rescue Exception` returning `""`, so
//! every message body rendered EMPTY behind a 200 and a green suite.
//! `scripts/campfire-cable-walk` is what found it.
//!
//! So a class-side method on a class the tree SUBCLASSES is
//! MONOMORPHIZED: a copy lands in every descendant that does not define
//! the name itself, each copy's `new` bound to the class it now lives
//! in, and the original keeps the owner. `Filter.apply` builds a
//! `Filter`; `SanitizeAttributes.apply` builds a `SanitizeAttributes`;
//! neither needs a dispatch at run time.
//!
//! WHY NOT `self.new`, WHICH IS WHAT THE SOURCE SAYS. It was tried, and
//! it is one arity short on spinel: `self.new` with NO arguments
//! compiles and dispatches to the receiver, `self.new(x)` is
//! `unsupported call: CallNode 'new' recv=SelfNode` (filed upstream).
//! Beyond that gap the shape is the wrong one for this pipeline anyway
//! — a class-side virtual call is exactly what the strict targets have
//! no vtable for, and a lowering may not branch on the target. The
//! subclass set of an ingested tree is CLOSED and known here, which is
//! what makes copying available where late binding is not.
//!
//! It costs a copy of the method per descendant. The whole corpus has
//! ONE site (`ActionText::Content::Filter.apply`, four descendants), so
//! the bound is measured rather than assumed — and if a class ever
//! turns up with a large class-side method and many subclasses, this is
//! the pass that will say so.
//!
//! Only `new`. `create` / `create!` are the same shape on a MODEL class
//! body, but they are also association-scope constructors that
//! `lower::scope_chain` already claims at implicit self
//! (`SELF_CONSTRUCTORS`), and no corpus app writes one in a constant.
//! When one does, it belongs beside this — not in a second walk.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_class_body_new_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let subclassed = subclassed_names(app);

    // Copies FIRST, while every method body still holds its bare `new`:
    // the rewrite below is what binds a receiver, and a copy taken after
    // it would carry the base's name into the subclass.
    diags.extend(monomorphize_subclassed(app, &subclassed));

    for model in &mut app.models {
        let owner = model.name.0.as_str().to_string();
        for item in &mut model.body {
            match item {
                // A model's constants arrive as replayed class-body
                // code rather than on a field of their own. A constant
                // initializer runs ONCE, in the body of the class that
                // declares it, so `self` there is the owner however many
                // subclasses exist.
                ModelBodyItem::Unknown { expr, .. } => {
                    if matches!(&*expr.node, ExprNode::Assign { target: crate::expr::LValue::Const { .. }, .. }) {
                        rewrite(expr, &owner);
                    }
                }
                ModelBodyItem::Method { method, .. }
                    if matches!(method.receiver, MethodReceiver::Class) =>
                {
                    rewrite(&mut method.body, &owner);
                }
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        let owner = lc.name.0.as_str().to_string();
        for (_, value) in &mut lc.constants {
            rewrite(value, &owner);
        }
        for m in &mut lc.methods {
            if matches!(m.receiver, MethodReceiver::Class) {
                rewrite(&mut m.body, &owner);
            }
        }
    }
    diags
}

/// Copy an inherited class-side method that constructs into each
/// descendant, so the copy's `new` can be bound to the descendant.
///
/// LIBRARY CLASSES ONLY, and a MODEL that needs the same is reported
/// rather than quietly bound to the wrong class. STI is the model shape
/// that would reach it (`Rooms::Open < Room`), no app in the corpus has
/// one, and building a second copy of this walk over `ModelBodyItem`
/// for a case nothing exercises is how a lowering acquires an untested
/// arm. The diagnostic is what stops it being silent.
fn monomorphize_subclassed(
    app: &mut App,
    subclassed: &std::collections::HashSet<String>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    for m in &app.models {
        let owner = m.name.0.as_str();
        if !subclassed.contains(owner) {
            continue;
        }
        for item in &m.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            if !matches!(method.receiver, MethodReceiver::Class) || !constructs(&method.body) {
                continue;
            }
            let mut d = Diagnostic::unsupported(
                Span::synthetic(),
                None,
                "inherited class-side constructor",
                format!(
                    "`{}.{}` builds with a bare `new` and `{owner}` is subclassed in this \
                     tree, so a subclass calling it gets a `{owner}` where Ruby would give \
                     it a subclass instance. Only library classes are monomorphized here.",
                    owner,
                    method.name.as_str(),
                ),
            );
            d.severity = crate::diagnostic::Severity::Warning;
            diags.push(d);
        }
    }

    // name -> index, and parent -> children, over library classes.
    let index: std::collections::HashMap<String, usize> = app
        .library_classes
        .iter()
        .enumerate()
        .map(|(i, lc)| (lc.name.0.as_str().to_string(), i))
        .collect();
    let mut children: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for lc in &app.library_classes {
        if let Some(p) = &lc.parent {
            children
                .entry(p.0.as_str().to_string())
                .or_default()
                .push(lc.name.0.as_str().to_string());
        }
    }

    // (destination index, the method to add)
    let mut planned: Vec<(usize, crate::dialect::MethodDef)> = Vec::new();
    for lc in &app.library_classes {
        let owner = lc.name.0.as_str().to_string();
        if !subclassed.contains(owner.as_str()) {
            continue;
        }
        for m in &lc.methods {
            if !matches!(m.receiver, MethodReceiver::Class) || !constructs(&m.body) {
                continue;
            }
            plan_copies(&owner, m, &children, &index, &app.library_classes, &mut planned);
        }
    }

    for (idx, mut method) in planned {
        let dest = app.library_classes[idx].name.0.as_str().to_string();
        rewrite(&mut method.body, &dest);
        app.library_classes[idx].methods.push(method);
    }

    diags
}

/// Walk the descendants of `owner`, copying `method` into each one that
/// does not define the name itself.
///
/// A descendant that DOES define it shadows the inherited one, and every
/// class below that descendant inherits the shadow rather than this
/// method — so that subtree is skipped whole rather than walked past.
fn plan_copies(
    owner: &str,
    method: &crate::dialect::MethodDef,
    children: &std::collections::HashMap<String, Vec<String>>,
    index: &std::collections::HashMap<String, usize>,
    classes: &[crate::dialect::LibraryClass],
    planned: &mut Vec<(usize, crate::dialect::MethodDef)>,
) {
    let Some(kids) = children.get(owner) else { return };
    for kid in kids {
        let Some(&i) = index.get(kid.as_str()) else { continue };
        let shadows = classes[i].methods.iter().any(|m| {
            matches!(m.receiver, MethodReceiver::Class) && m.name == method.name
        });
        if shadows {
            continue;
        }
        planned.push((i, method.clone()));
        plan_copies(kid, method, children, index, classes, planned);
    }
}

/// Does this body construct with a bare `new`?
fn constructs(expr: &Expr) -> bool {
    if let ExprNode::Send { recv: None, method, .. } = &*expr.node {
        if method.as_str() == "new" {
            return true;
        }
    }
    let mut found = false;
    expr.node.for_each_child(&mut |c| {
        if constructs(c) {
            found = true;
        }
    });
    found
}

/// Every class in the tree that something else declares as its parent.
///
/// Read off `parent` rather than from an inheritance index, because the
/// question is exactly "could `self` here be something other than the
/// owner", and one recorded superclass edge is the whole of it. A class
/// subclassed only OUTSIDE the tree (a gem's) is not in here and does
/// not need to be: nothing outside the tree calls into it either.
fn subclassed_names(app: &App) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for m in &app.models {
        if let Some(p) = &m.parent {
            out.insert(p.0.as_str().to_string());
        }
    }
    for lc in &app.library_classes {
        if let Some(p) = &lc.parent {
            out.insert(p.0.as_str().to_string());
        }
    }
    for c in &app.controllers {
        if let Some(p) = &c.parent {
            out.insert(p.0.as_str().to_string());
        }
    }
    out
}

fn rewrite(expr: &mut Expr, owner: &str) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, owner));
    let ExprNode::Send { recv: recv @ None, method, .. } = &mut *expr.node else { return };
    if method.as_str() != "new" {
        return;
    }
    let span = expr.span;
    let path: Vec<Symbol> = owner.split("::").map(Symbol::from).collect();
    let mut konst = Expr::new(span, ExprNode::Const { path });
    // The receiver is the class ITSELF, which is what the expression's
    // own type already says the result is — no new type is invented
    // here, and the send keeps whatever analyze stamped on it.
    konst.ty = expr.ty.clone().map(|t| match t {
        crate::ty::Ty::Class { id, args } => crate::ty::Ty::Class { id, args },
        _ => crate::ty::Ty::Untyped,
    });
    if matches!(konst.ty, Some(crate::ty::Ty::Untyped)) {
        konst.ty = None;
    }
    *recv = Some(konst);
}
