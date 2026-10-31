//! `alias to from` naming a method an app ancestor defines rather than
//! the class body itself.
//!
//! graphql-client 0.25.0's `OperationDefinition < Definition` does
//! `alias operation_name definition_name`, with `definition_name`
//! defined in definition.rb. A class body only knows its own methods as
//! it is walked, so the support walk has the ingest capture such an
//! alias as the `alias_method` call it is ([`capturing`]) and notes it.
//! Once the walk is done, [`settle`] looks each original up the way
//! Ruby does when the alias runs, along the superclass chain, each
//! class's methods before its includes (when the class's own includes
//! are app modules above the alias that do not define it): a file whose
//! every such alias resolves is carried, one with an original no app
//! superclass defines (a gem's or a core class's method) is refused at
//! the alias, as the ingest refuses it elsewhere.
//!
//! After the passes that make methods, [`resolve`] gives each class the
//! copy an alias of its own method is, under the alias's name, so
//! analysis and the other targets see an ordinary method. The Ruby
//! family writes the captured call back instead
//! (`crate::expr::BODY_STATEMENT`), which Ruby resolves through the
//! ancestors as the source's alias did: [`drop_replayed_copies`].

use std::cell::{Cell, RefCell};
use std::ops::Range;

use crate::app::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

thread_local! {
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    static DEFERRED: RefCell<Vec<Span>> = const { RefCell::new(Vec::new()) };
}

/// The message an alias naming a method nothing defines refuses its
/// file with.
pub(super) const REFUSAL: &str = "alias names a method this body has not defined";

/// Runs `ingest` with keyword `alias`es of methods the body does not
/// define captured rather than refused. Returns its result with the
/// captured aliases' spans.
pub(super) fn capturing<T>(ingest: impl FnOnce() -> T) -> (T, Vec<Span>) {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            CAPTURING.with(|c| c.set(self.0));
        }
    }
    let before = DEFERRED.with(|d| d.borrow().len());
    let reset = Reset(CAPTURING.with(|c| c.replace(true)));
    let out = ingest();
    drop(reset);
    let deferred = DEFERRED.with(|d| d.borrow_mut().split_off(before));
    (out, deferred)
}

pub(super) fn is_capturing() -> bool {
    CAPTURING.with(Cell::get)
}

/// How many aliases have been captured so far, for a body that must
/// not capture any ([`captured_since`]).
pub(super) fn captured() -> usize {
    DEFERRED.with(|d| d.borrow().len())
}

/// The first alias captured after `count` ([`captured`]).
pub(super) fn captured_since(count: usize) -> Option<Span> {
    DEFERRED.with(|d| d.borrow().get(count).copied())
}

/// `alias to from` at `span`, captured as `alias_method :to, :from`.
pub(super) fn capture(to: &str, from: &str, span: Span) -> Expr {
    DEFERRED.with(|d| d.borrow_mut().push(span));
    let sym = |name: &str| Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } });
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("alias_method"),
            args: vec![sym(to), sym(from)],
            block: None,
            parenthesized: false,
        },
    )
}

/// A support file whose classes hold captured aliases: its path, the
/// range of `App::library_classes` its classes went to, and the
/// aliases' spans.
pub(super) struct Held {
    pub(super) path: String,
    pub(super) classes: Range<usize>,
    pub(super) spans: Vec<Span>,
}

/// Keeps the held files whose every captured alias names a method an
/// app ancestor defines, among the classes of the files not held and
/// those of the held files found carried. The rest are taken back out
/// of `app` and refused at the first alias that does not resolve.
pub(super) fn settle(app: &mut App, held: Vec<Held>) {
    if held.is_empty() {
        return;
    }
    let unresolved = |app: &App, carried: &[bool], i: usize| -> Option<Span> {
        let excluded: Vec<&Range<usize>> =
            held.iter().enumerate().filter(|(j, _)| *j != i && !carried[*j]).map(|(_, h)| &h.classes).collect();
        let candidates: Vec<&LibraryClass> = app
            .library_classes
            .iter()
            .enumerate()
            .filter(|(k, _)| !excluded.iter().any(|range| range.contains(k)))
            .map(|(_, lc)| lc)
            .collect();
        held[i].spans.iter().copied().find(|span| {
            !app.library_classes[held[i].classes.clone()].iter().any(|lc| {
                lc.unknown_calls.iter().any(|call| {
                    call.span == *span
                        && alias_call(call).is_some_and(|(receiver, _, from)| {
                            ancestor_method(&candidates, lc, from, receiver, call.span).is_some()
                        })
                })
            })
        })
    };
    let mut carried = vec![false; held.len()];
    loop {
        let mut changed = false;
        for i in 0..held.len() {
            if !carried[i] && unresolved(app, &carried, i).is_none() {
                carried[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut refused: Vec<(usize, Span)> =
        (0..held.len()).filter(|i| !carried[*i]).filter_map(|i| Some((i, unresolved(app, &carried, i)?))).collect();
    refused.sort_by_key(|(i, _)| std::cmp::Reverse(held[*i].classes.start));
    for (i, span) in refused {
        let Held { path, classes, .. } = &held[i];
        app.library_classes.drain(classes.clone());
        app.library_class_files.retain(|(file, _)| file != path);
        app.file_programs.retain(|program| program.source != *path);
        if super::survey::is_active() {
            super::survey::record(&super::IngestError::Unsupported { file: path.clone(), message: REFUSAL.into() });
        }
        app.ingest_drops.push(crate::app::IngestDrop { span, message: REFUSAL.into() });
    }
}

/// Gives each class an alias of an ancestor's method resolves to the
/// copy of that method under the alias's name: what analysis and the
/// targets other than the Ruby family see.
pub(super) fn resolve(app: &mut App) {
    let mut copies: Vec<(usize, MethodDef)> = Vec::new();
    {
        let candidates: Vec<&LibraryClass> = app.library_classes.iter().collect();
        for (i, lc) in app.library_classes.iter().enumerate() {
            for call in &lc.unknown_calls {
                let Some((receiver, to, from)) = alias_call(call) else { continue };
                // A def of the alias's own name is the class's, later in
                // the body or another file of it, and one of the
                // original's is written below the alias, which aliases
                // the ancestor's: neither is the alias the copy
                // stands for.
                let defines = |name: &str| {
                    candidates
                        .iter()
                        .filter(|c| c.name == lc.name)
                        .flat_map(|c| c.methods.iter())
                        .any(|m| m.name.as_str() == name && m.receiver == receiver)
                };
                if defines(to) || defines(from) {
                    continue;
                }
                let Some(original) = ancestor_method(&candidates, lc, from, receiver, call.span) else { continue };
                // A renamed copy's `super` would reach the ancestor's
                // method of the alias's name, not the original's.
                if calls_super(&original.body) {
                    continue;
                }
                let mut copy = original.clone();
                copy.name = Symbol::from(to);
                copy.name_span = call.span;
                copy.body.decisions |= crate::expr::INHERITED_ALIAS_COPY;
                copies.push((i, copy));
            }
        }
    }
    for (i, copy) in copies {
        app.library_classes[i].methods.push(copy);
    }
}

/// The Ruby family's classes without the copies [`resolve`] made for
/// an alias the tree writes back as the captured call.
pub(crate) fn drop_replayed_copies(classes: &mut [LibraryClass]) {
    for lc in classes {
        let replayed: Vec<(MethodReceiver, String)> = lc
            .unknown_calls
            .iter()
            .filter(|call| call.decisions & crate::expr::BODY_STATEMENT != 0)
            .filter_map(|call| alias_call(call).map(|(receiver, to, _)| (receiver, to.to_string())))
            .collect();
        lc.methods.retain(|m| {
            m.body.decisions & crate::expr::INHERITED_ALIAS_COPY == 0
                || !replayed.iter().any(|(receiver, to)| *receiver == m.receiver && to == m.name.as_str())
        });
    }
}

/// Whether `call` is an alias [`resolve`] made `lc`'s copy for, which
/// carries it where the call is not written back.
pub(crate) fn carried_by_copy(lc: &LibraryClass, call: &Expr) -> bool {
    alias_call(call).is_some_and(|(receiver, to, _)| {
        lc.methods.iter().any(|m| {
            m.body.decisions & crate::expr::INHERITED_ALIAS_COPY != 0 && m.receiver == receiver && m.name.as_str() == to
        })
    })
}

/// `alias_method :to, :from` in a class body: on the class side when it
/// is written in the class's own `class << self`.
fn alias_call(call: &Expr) -> Option<(MethodReceiver, &str, &str)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else { return None };
    if method.as_str() != "alias_method" {
        return None;
    }
    let receiver = if call.decisions & crate::expr::SINGLETON_BODY != 0 {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };
    let [to, from] = args.as_slice() else { return None };
    Some((receiver, symbol(to)?, symbol(from)?))
}

fn symbol(arg: &Expr) -> Option<&str> {
    match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
        _ => None,
    }
}

fn calls_super(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Super { .. }) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |c| found = found || calls_super(c));
    found
}

fn named<'a>(classes: &[&'a LibraryClass], id: &str) -> Vec<&'a LibraryClass> {
    classes.iter().copied().filter(|c| c.name.0.as_str() == id).collect()
}

/// The first definition of `name` on `receiver`'s side along `lc`'s app
/// superclasses, each one's methods before its includes, for the alias
/// at `at`. `lc`'s own includes must all be app modules written above
/// the alias that do not define `name`: Ruby aliases a method of an
/// include above it, and not of one below, which the Ruby tree writes
/// above it. A `prepend`, or an include in another file of `lc`, leaves
/// the alias unresolved.
fn ancestor_method<'a>(
    app: &[&'a LibraryClass],
    lc: &LibraryClass,
    name: &str,
    receiver: MethodReceiver,
    at: Span,
) -> Option<&'a MethodDef> {
    if named(app, lc.name.0.as_str()).into_iter().any(|c| !std::ptr::eq(c, lc) && !c.includes.is_empty()) {
        return None;
    }
    let prepends = lc.unknown_calls.iter().any(|call| {
        matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "prepend")
    });
    if prepends || lc.includes.len() != lc.include_spans.len() {
        return None;
    }
    for (module, span) in lc.includes.iter().zip(&lc.include_spans) {
        if span.is_synthetic() || span.file != at.file || span.start > at.start {
            return None;
        }
        let modules: Vec<&LibraryClass> = app.iter().copied().filter(|c| c.is_module && c.name == *module).collect();
        if modules.is_empty() || modules.iter().flat_map(|c| c.methods.iter()).any(|m| m.name.as_str() == name) {
            return None;
        }
    }
    let own = |id: &str| -> Option<&'a MethodDef> {
        named(app, id).into_iter().flat_map(|c| c.methods.iter()).filter(|m| m.name.as_str() == name && m.receiver == receiver).last()
    };
    let included = |id: &str| -> Option<&'a MethodDef> {
        if receiver != MethodReceiver::Instance {
            return None;
        }
        let includes: Vec<&ClassId> = named(app, id).into_iter().flat_map(|c| c.includes.iter()).collect();
        includes.into_iter().rev().find_map(|module| {
            app.iter()
                .filter(|c| c.is_module && c.name == *module)
                .flat_map(|c| c.methods.iter())
                .filter(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
                .last()
        })
    };
    let mut seen: Vec<String> = vec![lc.name.0.as_str().to_string()];
    let mut class = lc.name.0.as_str().to_string();
    let mut parent = named(app, &class).into_iter().filter(|c| !c.is_module).find_map(|c| c.parent.clone());
    while let Some(written) = parent {
        let resolved = superclass(app, &class, written.0.as_str())?;
        if seen.contains(&resolved) || seen.len() > 32 {
            return None;
        }
        if let Some(found) = own(&resolved).or_else(|| included(&resolved)) {
            return Some(found);
        }
        seen.push(resolved.clone());
        parent = named(app, &resolved).into_iter().filter(|c| !c.is_module).find_map(|c| c.parent.clone());
        class = resolved;
    }
    None
}

/// The library class `written` names as `class`'s superclass: looked up
/// from the namespace the `class` statement is in outwards, then at the
/// top level.
fn superclass(app: &[&LibraryClass], class: &str, written: &str) -> Option<String> {
    if let Some(absolute) = written.strip_prefix("::") {
        return app.iter().any(|c| !c.is_module && c.name.0.as_str() == absolute).then(|| absolute.to_string());
    }
    let scope: Vec<&str> = class.split("::").collect();
    let is_class = |name: &str| app.iter().any(|c| !c.is_module && c.name.0.as_str() == name);
    (0..scope.len())
        .rev()
        .map(|depth| if depth == 0 { written.to_string() } else { format!("{}::{written}", scope[..depth].join("::")) })
        .find(|candidate| is_class(candidate))
}
