//! The part of a gem compiled from source (`ingest::auto_source_gems`)
//! that the program reaches, for the Spinel tree.
//!
//! Spinel compiles every class and method a tree carries, so a gem's code
//! the app never runs still has to compile there. parslet's `Context`
//! defines methods on each instance (`singleton_class.send(:define_method,
//! …)`), a shape spinel refuses, and lobsters never reaches it: only
//! `Transform#apply` builds one, and lobsters parses without transforming.
//!
//! Reached is decided on the analyzed IR, from the app's own code:
//!
//! - a class is reached when a reached body names it (a constant), or it
//!   is the parent, an included module or an enclosing namespace of a
//!   reached class;
//! - a call reaches the method the receiver's class answers it with: the
//!   named class's class method for `Const.m`, the receiver's type for a
//!   typed receiver, and `self`'s class and its reached descendants for a
//!   receiverless call. A receiver of unknown type reaches the method of
//!   that name on every reached class, so a dispatch the analysis cannot
//!   pin keeps too much rather than too little; but a class nothing names
//!   (`Transform`) is not one of them.
//!
//! A gem class nothing reaches is left out; a reached one keeps its
//! reached methods, its accessors, and the methods Ruby calls without a
//! call site (`initialize`, `to_s`, `==`, …).
//!
//! Every rule only adds, so reach is their least fixpoint, and a worklist
//! gets there pairing each class with each call once: a call is answered
//! by the classes reached so far, and a class, once reached, answers the
//! calls made so far.

use std::rc::Rc;

use crate::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{AccessorKind, LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::span::FileId;
use crate::ty::Ty;

/// Methods Ruby dispatches without a textual call site.
const IMPLICIT: &[&str] = &[
    "initialize", "initialize_copy", "initialize_dup", "initialize_clone", "to_s", "to_str", "to_a", "to_ary",
    "to_h", "to_hash", "to_proc", "to_sym", "to_i", "to_int", "to_f", "inspect", "hash", "eql?", "==", "===",
    "<=>", "each", "call", "coerce", "method_missing", "respond_to_missing?", "inherited", "included",
    "extended", "prepended", "method_added", "freeze", "dup", "clone",
];

/// A call by a name built at run time; the suffix the name ends with follows.
const DYNAMIC: &str = "\u{0}send ";

#[derive(Clone, PartialEq, Eq, Hash)]
enum Target {
    /// A receiver of unknown type.
    Any,
    /// `self`'s or a typed receiver's class (and its reached descendants);
    /// `true` for the class side.
    Virtual(String, bool),
    /// `super` from a method of this class.
    Super(String, bool),
}

type MethodKey = (String, bool, String);
type Call = (Target, String);
type Const = (Option<String>, String);

pub(crate) fn shake(app: &App) -> Option<App> {
    let gems = Gems::of(app)?;
    let reached = gems.reach(app);
    gems.trim(app, &reached)
}

/// What the program reaches: classes by name, methods by owner, side and name.
#[derive(Debug, PartialEq, Eq)]
struct Reached {
    classes: HashSet<String>,
    methods: HashSet<MethodKey>,
}

fn mounted(app: &App, file: FileId) -> bool {
    let Some(lock) = app.gem_lock.as_ref() else { return false };
    app.source_path(file).is_some_and(|path| {
        app.auto_source_gems
            .iter()
            .any(|gem| lock.version_of(gem).is_some_and(|v| crate::ingest::source_gems::file_of(path, gem, v)))
    })
}

fn file_of_class(c: &LibraryClass) -> Option<FileId> {
    // The class's own header first: a macro the gem defines (parslet's `rule`) leaves its expanded methods' spans in the gem.
    std::iter::once(c.parent_span.file)
        .chain(c.include_spans.iter().map(|span| span.file))
        .chain(c.constants.iter().map(|(_, value)| value.span.file))
        .chain(c.methods.iter().map(|m| m.body.span.file))
        .find(|file| file.0 != 0)
}

/// The gems' classes and the class graph reach walks; none of it changes as reach grows.
struct Gems<'a> {
    names: HashSet<String>,
    entries: HashMap<String, Vec<&'a LibraryClass>>,
    /// `(owner, class side, method)` for every method a class defines.
    defined: HashSet<(&'a str, bool, &'a str)>,
    /// The classes defining a method of this name, on either side.
    owners: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> Gems<'a> {
    fn of(app: &'a App) -> Option<Self> {
        if app.auto_source_gems.is_empty() {
            return None;
        }
        app.gem_lock.as_ref()?;
        let mut names: HashSet<String> = app
            .library_classes
            .iter()
            .filter(|c| file_of_class(c).is_some_and(|file| mounted(app, file)))
            .map(|c| c.name.0.to_string())
            .collect();
        if names.is_empty() {
            return None;
        }
        // Not the app's: a partless reopening (`class Expression` only enclosing `Treetop`) has no file, but its name is the gem's.
        let partless: Vec<String> = app
            .library_classes
            .iter()
            .filter(|c| file_of_class(c).is_none())
            .map(|c| c.name.0.to_string())
            .filter(|n| names.contains(n) || names.iter().any(|g| g.starts_with(&format!("{n}::")) || n.starts_with(&format!("{g}::"))))
            .collect();
        names.extend(partless);
        let mut entries: HashMap<String, Vec<&LibraryClass>> = HashMap::default();
        let mut defined = HashSet::default();
        let mut owners: HashMap<&str, Vec<&str>> = HashMap::default();
        for c in &app.library_classes {
            entries.entry(c.name.0.to_string()).or_default().push(c);
            for m in &c.methods {
                if defined.insert((c.name.0.as_str(), m.receiver == MethodReceiver::Class, m.name.as_str())) {
                    let by_name = owners.entry(m.name.as_str()).or_default();
                    if by_name.last() != Some(&c.name.0.as_str()) {
                        by_name.push(c.name.0.as_str());
                    }
                }
            }
        }
        Some(Gems { names, entries, defined, owners })
    }

    fn is_gem(&self, c: &LibraryClass) -> bool {
        self.names.contains(c.name.0.as_str())
    }

    fn known(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// A name read inside `context` may be any enclosing namespace's (`Atoms::Entity` in `Parslet`).
    fn resolve(&self, context: Option<&str>, name: &str) -> Option<String> {
        let name = name.trim_start_matches("::");
        let mut scope = context.unwrap_or("");
        loop {
            if !scope.is_empty() {
                let candidate = format!("{scope}::{name}");
                if self.known(&candidate) {
                    return Some(candidate);
                }
            }
            match scope.rsplit_once("::") {
                Some((outer, _)) => scope = outer,
                None if !scope.is_empty() => scope = "",
                None => break,
            }
        }
        self.known(name).then(|| name.to_string())
    }

    fn parent_of(&self, name: &str) -> Option<String> {
        self.entries.get(name)?.iter().find_map(|c| c.parent.as_ref()).and_then(|p| self.resolve(Some(name), p.0.as_str()))
    }

    fn includes_of(&self, name: &str) -> Vec<String> {
        self.entries
            .get(name)
            .into_iter()
            .flatten()
            .flat_map(|c| c.includes.iter())
            .filter_map(|m| self.resolve(Some(name), m.0.as_str()))
            .collect()
    }

    /// Method resolution order, as Ruby looks a method up: the class, its modules, then its parent's.
    fn ancestors(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut current = Some(name.to_string());
        let mut guard = 0;
        while let Some(c) = current {
            guard += 1;
            if guard > 64 || out.contains(&c) {
                break;
            }
            out.push(c.clone());
            let mut stack = self.includes_of(&c);
            stack.reverse();
            while let Some(m) = stack.pop() {
                if !out.contains(&m) {
                    out.push(m.clone());
                    let mut more = self.includes_of(&m);
                    more.reverse();
                    stack.extend(more);
                }
            }
            current = self.parent_of(&c);
        }
        out
    }

    /// What loads with a reached class: its parent, its modules and the namespaces enclosing it.
    fn loads_with(&self, c: &str) -> Vec<String> {
        let mut more: Vec<String> = Vec::new();
        more.extend(self.parent_of(c));
        more.extend(self.includes_of(c));
        let mut outer = c;
        while let Some((o, _)) = outer.rsplit_once("::") {
            if self.known(o) {
                more.push(o.to_string());
            }
            outer = o;
        }
        more
    }

    /// The bodies outside the gems, and the app's own classes, `self` known in their methods.
    fn roots(&self, app: &App) -> (Vec<Call>, Vec<Const>, Vec<String>) {
        let mut s = Scan { context: None, calls: Vec::new(), consts: Vec::new() };
        crate::lower::for_each_emit_body_ref(app, &mut |e| {
            if e.span.file.0 == 0 || !mounted(app, e.span.file) {
                scan(e, &mut s);
            }
        });
        let (mut calls, mut consts) = (s.calls, s.consts);
        let mut live = Vec::new();
        for c in app.library_classes.iter().filter(|c| !self.is_gem(c)) {
            live.push(c.name.0.to_string());
            let name = c.name.0.as_str();
            for m in &c.methods {
                let mut s = Scan { context: Some((name, m.receiver == MethodReceiver::Class, m.name.as_str())), calls: Vec::new(), consts: Vec::new() };
                scan(&m.body, &mut s);
                calls.extend(s.calls);
                consts.extend(s.consts);
            }
        }
        for m in &app.models {
            if let Some(p) = &m.parent {
                consts.push((None, p.0.to_string()));
            }
        }
        (calls, consts, live)
    }

    /// A reached gem class's body runs as it loads, and Ruby may call its implicit methods at any time.
    fn class_body(&self, c: &str) -> (Vec<MethodKey>, Vec<Call>, Vec<Const>) {
        let mut implicit = Vec::new();
        for entry in self.entries.get(c).into_iter().flatten() {
            for m in entry.methods.iter().filter(|m| IMPLICIT.contains(&m.name.as_str())) {
                implicit.push((c.to_string(), m.receiver == MethodReceiver::Class, m.name.to_string()));
            }
        }
        let mut s = Scan { context: Some((c, true, "")), calls: Vec::new(), consts: Vec::new() };
        for entry in self.entries.get(c).into_iter().flatten() {
            for (_, value) in &entry.constants {
                scan(value, &mut s);
            }
            for call in &entry.unknown_calls {
                scan(call, &mut s);
            }
        }
        (implicit, s.calls, s.consts)
    }

    /// A reached gem method's body is reached code.
    fn method_body(&self, (owner, side, name): &MethodKey) -> (Vec<Call>, Vec<Const>) {
        let mut s = Scan { context: Some((owner.as_str(), *side, name.as_str())), calls: Vec::new(), consts: Vec::new() };
        for entry in self.entries.get(owner).into_iter().flatten() {
            for m in entry.methods.iter().filter(|m| m.name.as_str() == name && (m.receiver == MethodReceiver::Class) == *side) {
                scan(&m.body, &mut s);
                for p in m.params.iter().filter_map(|p| p.default.as_ref()) {
                    scan(p, &mut s);
                }
            }
        }
        (s.calls, s.consts)
    }

    /// The methods of `c`'s own entries a built name ending in `suffix` may call.
    fn suffixed(&self, c: &str, suffix: &str) -> Vec<String> {
        self.entries
            .get(c)
            .into_iter()
            .flatten()
            .flat_map(|entry| entry.methods.iter())
            .filter(|m| m.name.as_str().ends_with(suffix))
            .map(|m| m.name.to_string())
            .collect()
    }

    fn reach(&self, app: &App) -> Reached {
        let mut walk = Walk {
            gems: self,
            ancestors: HashMap::default(),
            classes: HashSet::default(),
            methods: HashSet::default(),
            calls: HashSet::default(),
            new_classes: Vec::new(),
            new_methods: Vec::new(),
            new_calls: Vec::new(),
            consts: Vec::new(),
            descendants: HashMap::default(),
            any: HashSet::default(),
            virtuals: HashMap::default(),
            dynamic: Vec::new(),
        };
        let (calls, consts, live) = self.roots(app);
        for c in live {
            walk.class(c);
        }
        for call in calls {
            walk.call(call);
        }
        walk.consts = consts;
        walk.run();
        Reached { classes: walk.classes, methods: walk.methods }
    }

    fn trim(&self, app: &App, reached: &Reached) -> Option<App> {
        let mut trimmed = app.clone();
        let before: usize = trimmed.library_classes.iter().map(|c| c.methods.len() + 1).sum();
        trimmed.library_classes.retain(|c| !self.is_gem(c) || reached.classes.contains(c.name.0.as_str()));
        for c in trimmed.library_classes.iter_mut().filter(|c| self.names.contains(c.name.0.as_str())) {
            let owner = c.name.0.to_string();
            c.methods.retain(|m| {
                matches!(m.kind, AccessorKind::AttributeReader | AccessorKind::AttributeWriter)
                    || IMPLICIT.contains(&m.name.as_str())
                    || reached.methods.contains(&(owner.clone(), m.receiver == MethodReceiver::Class, m.name.to_string()))
            });
        }
        let after: usize = trimmed.library_classes.iter().map(|c| c.methods.len() + 1).sum();
        (after != before).then_some(trimmed)
    }
}

/// Reach as it grows. A class waits in `new_classes` until it has answered
/// the calls made so far, a call in `new_calls` until the classes reached
/// so far have answered it, so each pair is met once, by whichever of the
/// two comes second.
struct Walk<'g, 'a> {
    gems: &'g Gems<'a>,
    ancestors: HashMap<String, Rc<[String]>>,
    classes: HashSet<String>,
    methods: HashSet<MethodKey>,
    calls: HashSet<Call>,
    new_classes: Vec<String>,
    new_methods: Vec<MethodKey>,
    new_calls: Vec<Call>,
    consts: Vec<Const>,
    /// The classes past `new_classes`, under each of their ancestors.
    descendants: HashMap<String, Vec<String>>,
    /// The method names called on a receiver of unknown type.
    any: HashSet<String>,
    /// The method names called on a class and its descendants, by class and side.
    virtuals: HashMap<(String, bool), HashSet<String>>,
    /// The calls by a built name: their target and the suffix the name ends with.
    dynamic: Vec<(Target, String)>,
}

impl Walk<'_, '_> {
    fn run(&mut self) {
        loop {
            if let Some((context, name)) = self.consts.pop() {
                if let Some(c) = self.gems.resolve(context.as_deref(), &name) {
                    self.class(c);
                }
            } else if let Some(c) = self.new_classes.pop() {
                self.answer_with_class(c);
            } else if let Some(call) = self.new_calls.pop() {
                self.answer_call(call);
            } else if let Some(key) = self.new_methods.pop() {
                if self.gems.names.contains(&key.0) {
                    let (calls, consts) = self.gems.method_body(&key);
                    self.found(calls, consts);
                }
            } else {
                break;
            }
        }
    }

    fn class(&mut self, c: String) {
        if self.classes.insert(c.clone()) {
            self.new_classes.push(c);
        }
    }

    fn method(&mut self, key: MethodKey) {
        if self.methods.insert(key.clone()) {
            self.new_methods.push(key);
        }
    }

    fn call(&mut self, call: Call) {
        if self.calls.insert(call.clone()) {
            self.new_calls.push(call);
        }
    }

    fn found(&mut self, calls: Vec<Call>, consts: Vec<Const>) {
        for call in calls {
            self.call(call);
        }
        self.consts.extend(consts);
    }

    fn ancestors(&mut self, name: &str) -> Rc<[String]> {
        if let Some(found) = self.ancestors.get(name) {
            return found.clone();
        }
        let found: Rc<[String]> = self.gems.ancestors(name).into();
        self.ancestors.insert(name.to_string(), found.clone());
        found
    }

    /// The ancestor of `c` whose method of this name and side a call on `c` runs.
    fn owner(&mut self, c: &str, side: bool, method: &str) -> Option<String> {
        self.ancestors(c).iter().find(|a| self.gems.defined.contains(&(a.as_str(), side, method))).cloned()
    }

    fn answer(&mut self, c: &str, side: bool, method: &str) {
        if let Some(owner) = self.owner(c, side, method) {
            self.method((owner, side, method.to_string()));
        }
    }

    fn answer_with_class(&mut self, d: String) {
        let ancestors = self.ancestors(&d);
        for a in ancestors.iter() {
            self.descendants.entry(a.clone()).or_default().push(d.clone());
        }
        // Calls on a receiver of unknown type: the names `d` answers that one was made with.
        let gems = self.gems;
        for a in ancestors.iter() {
            for entry in gems.entries.get(a.as_str()).into_iter().flatten() {
                for m in &entry.methods {
                    if self.any.contains(m.name.as_str()) {
                        self.answer(&d, m.receiver == MethodReceiver::Class, m.name.as_str());
                    }
                }
            }
        }
        // Calls on one of its ancestors and the ancestor's descendants.
        for a in ancestors.iter() {
            for side in [false, true] {
                let Some(names) = self.virtuals.get(&(a.clone(), side)) else { continue };
                for method in names.clone() {
                    self.answer(&d, side, &method);
                }
            }
        }
        for (target, suffix) in self.dynamic.clone() {
            let answers = match &target {
                Target::Virtual(c, _) if !c.starts_with('\u{0}') => ancestors.contains(c),
                _ => true,
            };
            if answers {
                for name in gems.suffixed(&d, &suffix) {
                    self.call((target.clone(), name));
                }
            }
        }
        for c in gems.loads_with(&d) {
            self.class(c);
        }
        if gems.names.contains(&d) {
            let (implicit, calls, consts) = gems.class_body(&d);
            for key in implicit {
                self.method(key);
            }
            self.found(calls, consts);
        }
    }

    fn answer_call(&mut self, (target, method): Call) {
        let gems = self.gems;
        if let Some(suffix) = method.strip_prefix(DYNAMIC) {
            self.dynamic.push((target.clone(), suffix.to_string()));
            let classes: Vec<String> = match &target {
                Target::Virtual(c, _) if !c.starts_with('\u{0}') => self.descendants.get(c).cloned().unwrap_or_default(),
                _ => self.classes.iter().cloned().collect(),
            };
            for c in classes {
                for name in gems.suffixed(&c, suffix) {
                    self.call((target.clone(), name));
                }
            }
            return;
        }
        match target {
            Target::Any => {
                if !self.any.insert(method.clone()) {
                    return;
                }
                let mut receivers: Vec<String> = Vec::new();
                let mut seen: HashSet<&str> = HashSet::default();
                for owner in gems.owners.get(method.as_str()).into_iter().flatten() {
                    for d in self.descendants.get(*owner).into_iter().flatten() {
                        if seen.insert(d.as_str()) {
                            receivers.push(d.clone());
                        }
                    }
                }
                for d in receivers {
                    for side in [false, true] {
                        self.answer(&d, side, &method);
                    }
                }
            }
            Target::Super(c, side) => {
                let owner = self.ancestors(&c).iter().skip(1).find(|a| gems.defined.contains(&(a.as_str(), side, method.as_str()))).cloned();
                if let Some(owner) = owner {
                    self.method((owner, side, method));
                }
            }
            Target::Virtual(c, side) => {
                // A constant receiver: resolved where it was written.
                let c = match c.strip_prefix('\u{0}').and_then(|rest| rest.split_once('\u{0}')) {
                    Some((context, name)) => match gems.resolve((!context.is_empty()).then_some(context), name) {
                        Some(resolved) => resolved,
                        None => return,
                    },
                    None => c,
                };
                if !self.virtuals.entry((c.clone(), side)).or_default().insert(method.clone()) {
                    return;
                }
                let mut receivers = self.descendants.get(&c).cloned().unwrap_or_default();
                receivers.push(c);
                for d in receivers {
                    self.answer(&d, side, &method);
                }
            }
        }
    }
}

/// Walk a body: the calls it makes and the classes it names.
struct Scan<'a> {
    context: Option<(&'a str, bool, &'a str)>,
    calls: Vec<Call>,
    consts: Vec<Const>,
}

fn scan(e: &Expr, s: &mut Scan<'_>) {
    match &*e.node {
        ExprNode::Const { path } => {
            let name = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            s.consts.push((s.context.map(|c| c.0.to_string()), name));
        }
        ExprNode::Send { recv, method, args, .. } => {
            let target = match recv {
                None => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                Some(r) => match &*r.node {
                    ExprNode::SelfRef => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                    ExprNode::Const { path } => {
                        let name = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                        Target::Virtual(format!("\u{0}{}\u{0}{name}", s.context.map_or("", |c| c.0)), true)
                    }
                    _ => match r.ty.as_ref().map(Ty::peel_nilable) {
                        Some(Ty::Class { id, .. }) => Target::Virtual(id.0.to_string(), false),
                        _ => Target::Any,
                    },
                },
            };
            let by_name = matches!(method.as_str(), "send" | "public_send" | "__send__" | "method" | "public_method" | "respond_to?" | "instance_method");
            s.calls.push((target.clone(), method.as_str().to_string()));
            // `send(:name)`: a call by a name held as a Symbol.
            match args.first().map(|a| &*a.node) {
                Some(ExprNode::Lit { value: Literal::Sym { value } }) if by_name => {
                    s.calls.push((Target::Any, value.as_str().to_string()));
                }
                // Not dropped as unknown: svg-graph's `init_with` reaches its option writers by `send(key.to_s + "=", value)`.
                Some(name) if by_name && matches!(method.as_str(), "send" | "public_send" | "__send__") => {
                    let suffix = match name {
                        ExprNode::Send { method, args, .. } if method.as_str() == "+" => match args.first().map(|a| &*a.node) {
                            Some(ExprNode::Lit { value: Literal::Str { value } }) => value.to_string(),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    s.calls.push((target, format!("{DYNAMIC}{suffix}")));
                }
                _ => {}
            }
        }
        // `method(:name)` / `recv.method(:name)`: ingest's bound-method reference, a call by name later.
        ExprNode::MethodRef { recv, name } => {
            let target = match recv {
                None => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                Some(r) => match &*r.node {
                    ExprNode::SelfRef => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                    _ => Target::Any,
                },
            };
            s.calls.push((target, name.as_str().to_string()));
        }
        ExprNode::Super { .. } => {
            if let Some((c, side, m)) = s.context {
                s.calls.push((Target::Super(c.to_string(), side), m.to_string()));
            }
        }
        _ => {}
    }
    e.node.for_each_child(&mut |child| scan(child, s));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_target::BuildTarget;
    use std::path::{Path, PathBuf};

    /// The fixpoint the worklist replaced: every round walks every call
    /// against every reached class. Kept as the oracle the worklist must
    /// agree with.
    fn reference(gems: &Gems<'_>, app: &App) -> Reached {
        let gem_names = &gems.names;
        let entries = &gems.entries;
        let known = |name: &str| gems.known(name);
        let resolve = |context: Option<&str>, name: &str| gems.resolve(context, name);
        let parent_of = |name: &str| gems.parent_of(name);
        let includes_of = |name: &str| gems.includes_of(name);
        let ancestors = |name: &str| gems.ancestors(name);
        let defines = |owner: &str, class_side: bool, method: &str| {
            entries.get(owner).into_iter().flatten().any(|c| {
                c.methods.iter().any(|m| m.name.as_str() == method && (m.receiver == MethodReceiver::Class) == class_side)
            })
        };
        let (mut calls, mut consts, live) = gems.roots(app);
        let mut live_classes: HashSet<String> = live.into_iter().collect();
        let mut live_methods: HashSet<MethodKey> = HashSet::default();
        let mut scanned_methods: HashSet<MethodKey> = HashSet::default();
        let mut scanned_bodies: HashSet<String> = HashSet::default();
        loop {
            let mut changed = false;
            for (context, name) in std::mem::take(&mut consts) {
                if let Some(c) = resolve(context.as_deref(), &name)
                    && live_classes.insert(c)
                {
                    changed = true;
                }
            }
            loop {
                let mut more: Vec<String> = Vec::new();
                for c in &live_classes {
                    more.extend(parent_of(c));
                    more.extend(includes_of(c));
                    let mut outer = c.as_str();
                    while let Some((o, _)) = outer.rsplit_once("::") {
                        if known(o) {
                            more.push(o.to_string());
                        }
                        outer = o;
                    }
                }
                let before = live_classes.len();
                live_classes.extend(more);
                if live_classes.len() == before {
                    break;
                }
                changed = true;
            }
            for c in live_classes.clone() {
                if gem_names.contains(&c) && scanned_bodies.insert(c.clone()) {
                    for entry in entries.get(&c).into_iter().flatten() {
                        for m in entry.methods.iter().filter(|m| IMPLICIT.contains(&m.name.as_str())) {
                            live_methods.insert((c.clone(), m.receiver == MethodReceiver::Class, m.name.to_string()));
                        }
                    }
                    for entry in entries.get(&c).into_iter().flatten() {
                        let mut s = Scan { context: Some((c.as_str(), true, "")), calls: Vec::new(), consts: Vec::new() };
                        for (_, value) in &entry.constants {
                            scan(value, &mut s);
                        }
                        for call in &entry.unknown_calls {
                            scan(call, &mut s);
                        }
                        calls.extend(s.calls);
                        consts.extend(s.consts);
                    }
                    changed = true;
                }
            }
            let mut reached: Vec<MethodKey> = Vec::new();
            let mut named: Vec<(Target, String)> = Vec::new();
            for (target, method) in &calls {
                let Some(suffix) = method.strip_prefix(DYNAMIC) else { continue };
                let classes: Vec<&String> = match target {
                    Target::Virtual(c, _) if !c.starts_with('\u{0}') => live_classes.iter().filter(|d| ancestors(d).contains(c)).collect(),
                    _ => live_classes.iter().collect(),
                };
                for c in classes {
                    for entry in entries.get(c.as_str()).into_iter().flatten() {
                        for m in entry.methods.iter().filter(|m| m.name.as_str().ends_with(suffix)) {
                            named.push((target.clone(), m.name.to_string()));
                        }
                    }
                }
            }
            for (target, method) in calls.iter().filter(|(_, m)| !m.starts_with(DYNAMIC)).chain(named.iter()) {
                let (receivers, class_side): (Vec<String>, bool) = match target {
                    Target::Any => {
                        for c in &live_classes {
                            for side in [false, true] {
                                if let Some(owner) = ancestors(c).into_iter().find(|a| defines(a, side, method)) {
                                    reached.push((owner, side, method.clone()));
                                }
                            }
                        }
                        continue;
                    }
                    Target::Super(c, side) => {
                        if let Some(owner) = ancestors(c).into_iter().skip(1).find(|a| defines(a, *side, method)) {
                            reached.push((owner, *side, method.clone()));
                        }
                        continue;
                    }
                    Target::Virtual(c, side) => {
                        let c = match c.strip_prefix('\u{0}').and_then(|rest| rest.split_once('\u{0}')) {
                            Some((context, name)) => match resolve((!context.is_empty()).then_some(context), name) {
                                Some(resolved) => resolved,
                                None => continue,
                            },
                            None => c.clone(),
                        };
                        let descendants: Vec<String> =
                            live_classes.iter().filter(|d| ancestors(d).contains(&c)).cloned().chain(std::iter::once(c.clone())).collect();
                        (descendants, *side)
                    }
                };
                for d in receivers {
                    if let Some(owner) = ancestors(&d).into_iter().find(|a| defines(a, class_side, method)) {
                        reached.push((owner, class_side, method.clone()));
                    }
                }
            }
            for key in reached {
                if live_methods.insert(key.clone()) {
                    changed = true;
                }
            }
            for key in live_methods.clone() {
                let (owner, side, name) = &key;
                if !gem_names.contains(owner) || !scanned_methods.insert(key.clone()) {
                    continue;
                }
                for entry in entries.get(owner).into_iter().flatten() {
                    for m in entry.methods.iter().filter(|m| m.name.as_str() == name && (m.receiver == MethodReceiver::Class) == *side) {
                        let mut s = Scan { context: Some((owner.as_str(), *side, name.as_str())), calls: Vec::new(), consts: Vec::new() };
                        scan(&m.body, &mut s);
                        for p in m.params.iter().filter_map(|p| p.default.as_ref()) {
                            scan(p, &mut s);
                        }
                        calls.extend(s.calls);
                        consts.extend(s.consts);
                    }
                }
                changed = true;
            }
            if !changed {
                break;
            }
        }
        Reached { classes: live_classes, methods: live_methods }
    }

    const GEMS: &str = "vendor/bundle/ruby/3.4.0/gems";

    /// real-blog with `gems` installed under `vendor/bundle` and locked as
    /// the app's dependencies, `edits` applied, analyzed and lowered as
    /// the Spinel emit sees it.
    fn spinel_app(gems: &[(&str, &[&str])], files: &[(&str, &str)], edits: &[(&str, &str, &str)]) -> App {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "roundhouse-gem-shake-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_tree(crate::fixtures::real_blog(), &dir);
        let specs: String = gems
            .iter()
            .map(|(gem, deps)| format!("    {gem}\n{}", deps.iter().map(|d| format!("      {d}\n")).collect::<String>()))
            .collect();
        edit(&dir, "Gemfile.lock", "    debug (1.11.1)\n", &format!("{specs}    debug (1.11.1)\n"));
        let roots: String = gems
            .iter()
            .filter(|(gem, _)| !gems.iter().any(|(_, deps)| deps.contains(&gem.split(' ').next().unwrap())))
            .map(|(gem, _)| format!("  {}\n", gem.split(' ').next().unwrap()))
            .collect();
        edit(&dir, "Gemfile.lock", "DEPENDENCIES\n", &format!("DEPENDENCIES\n{roots}"));
        write(&dir, ".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        for (path, content) in files {
            write(&dir, &format!("{GEMS}/{path}"), content);
        }
        for (path, find, replace) in edits {
            edit(&dir, path, find, replace);
        }
        let (app, _) = crate::ingest::prism::scope(|| crate::ingest::ingest_app(&dir));
        let mut app = app.expect("ingest the gem fixture");
        let _ = std::fs::remove_dir_all(&dir);
        crate::session::analyze_and_lower_for(&mut app, BuildTarget::Spinel);
        app
    }

    fn copy_tree(src: &Path, dst: &Path) {
        if src.is_dir() {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if ["tmp", "log", "storage", "node_modules", ".git"].contains(&name.as_ref()) || name.starts_with(".bundle") {
                    continue;
                }
                copy_tree(&entry.path(), &dst.join(entry.file_name()));
            }
        } else {
            std::fs::copy(src, dst).unwrap();
        }
    }

    fn write(dir: &Path, path: &str, content: &str) {
        let full: PathBuf = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    fn edit(dir: &Path, path: &str, find: &str, replace: &str) {
        let full = dir.join(path);
        let text = std::fs::read_to_string(&full).unwrap();
        assert_eq!(text.matches(find).count(), 1, "{find:?} in {path}");
        std::fs::write(full, text.replacen(find, replace, 1)).unwrap();
    }

    fn reached(app: &App) -> Reached {
        let gems = Gems::of(app).expect("the app compiles a gem from source");
        let reached = gems.reach(app);
        assert_eq!(reached, reference(&gems, app), "the worklist and the fixpoint disagree");
        reached
    }

    fn method(owner: &str, class_side: bool, name: &str) -> MethodKey {
        (owner.to_string(), class_side, name.to_string())
    }

    /// `tests/support/auto_source_gem.rs`'s gems: a class only an unreached
    /// method builds, and writers reached by a name built at run time.
    #[test]
    fn the_worklist_reaches_what_the_fixpoint_did_in_the_auto_source_gem() {
        let app = spinel_app(
            &[("exclaim (0.1)", &[]), ("greetings (1.0)", &["exclaim"])],
            &[
                ("greetings-1.0/lib/greetings.rb", "require \"exclaim\"\nrequire \"greetings/formatter\"\nrequire \"greetings/bindings\"\nrequire \"greetings/card\"\n"),
                ("greetings-1.0/lib/greetings/formatter.rb", "module Greetings\n  class Formatter\n    def self.shout(text)\n      Exclaim.mark(text.upcase)\n    end\n\n    def self.bound(values)\n      Bindings.new(values)\n    end\n  end\nend\n"),
                ("greetings-1.0/lib/greetings/bindings.rb", "module Greetings\n  class Bindings\n    def initialize(values)\n      values.each do |key, value|\n        singleton_class.send(:define_method, key) { value }\n      end\n    end\n  end\nend\n"),
                ("greetings-1.0/lib/greetings/card.rb", "module Greetings\n  class Panel\n    attr_accessor :width\n\n    def initialize(options)\n      options.each { |key, value| send(key.to_s + \"=\", value) }\n    end\n  end\n\n  class Card < Panel\n    protected\n\n    def width=(value)\n      @width = value.to_i * 2\n    end\n  end\nend\n"),
                ("exclaim-0.1/lib/exclaim.rb", "module Exclaim\n  def self.mark(text)\n    text + \"!\"\n  end\nend\n"),
            ],
            &[(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  def shouted_title\n    Greetings::Formatter.shout(title)\n  end\n\n  def card_width\n    Greetings::Card.new({ width: \"7\" }).width\n  end\n\n",
            )],
        );
        let reached = reached(&app);
        assert!(reached.methods.contains(&method("Greetings::Formatter", true, "shout")), "{reached:#?}");
        assert!(reached.methods.contains(&method("Exclaim", true, "mark")), "{reached:#?}");
        assert!(reached.methods.contains(&method("Greetings::Card", false, "width=")), "{reached:#?}");
        assert!(!reached.methods.contains(&method("Greetings::Formatter", true, "bound")), "{reached:#?}");
        assert!(!reached.classes.contains("Greetings::Bindings"), "{reached:#?}");
    }

    /// Dispatch through an included module, `super`, `method(:name)`,
    /// `send(:name)` and a receiver of unknown type, which reaches a name
    /// on the reached classes only, including the ones reached after it.
    #[test]
    fn the_worklist_reaches_what_the_fixpoint_did_through_modules_super_and_untyped_receivers() {
        let app = spinel_app(
            &[("tally (1.0)", &[])],
            &[
                ("tally-1.0/lib/tally.rb", "require \"tally/counting\"\nrequire \"tally/base\"\nrequire \"tally/meter\"\nrequire \"tally/spare\"\nrequire \"tally/dial\"\n"),
                // Named only two calls deep in the gem, so reached after the app's calls were answered.
                ("tally-1.0/lib/tally/dial.rb", "module Tally\n  class Dial\n    def turn=(value)\n      value\n    end\n  end\n\n  class Knob < Dial\n    def turn=(value)\n      value * 2\n    end\n\n    def spin=(value)\n      value\n    end\n\n    def reading\n      3\n    end\n  end\n\n  class Heavy < Base\n    def self.build(value)\n      value\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/counting.rb", "module Tally\n  module Counting\n    def count_up(n)\n      step(n) + 1\n    end\n\n    def step(n)\n      n\n    end\n\n    def count_down(n)\n      n - 1\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/base.rb", "module Tally\n  class Base\n    include Counting\n\n    def self.build(value)\n      new.measure(value)\n    end\n\n    def measure(value)\n      value.to_s\n    end\n\n    def label\n      \"base\"\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/meter.rb", "module Tally\n  class Meter < Base\n    def measure(value)\n      super + label\n    end\n\n    def label\n      tag = method(:suffix)\n      tag.call + send(:prefix)\n    end\n\n    def suffix\n      \"m\"\n    end\n\n    def prefix\n      \"p\"\n    end\n\n    def readings(items)\n      items.map { |item| item.reading }\n    end\n\n    def reading\n      1\n    end\n\n    def kinds\n      spare_kinds\n    end\n\n    def spare_kinds\n      [Knob, Heavy]\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/spare.rb", "module Tally\n  class Spare\n    def reading\n      2\n    end\n  end\nend\n"),
            ],
            &[(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  def tally\n    Tally::Meter.build(title)\n  end\n\n  def counted\n    Tally::Meter.new.count_up(2)\n  end\n\n  def readings(items)\n    Tally::Meter.new.readings(items)\n  end\n\n  def kinds\n    Tally::Meter.new.kinds\n  end\n\n  def first_reading(items)\n    items.first.reading\n  end\n\n  def tune(name)\n    Tally::Dial.new.send(name.to_s + \"=\", 1)\n  end\n\n",
            )],
        );
        let reached = reached(&app);
        for (owner, class_side, name) in [
            ("Tally::Knob", false, "reading"),
            ("Tally::Knob", false, "turn="),
            ("Tally::Knob", false, "spin="),
            ("Tally::Heavy", true, "build"),
            ("Tally::Base", true, "build"),
            ("Tally::Meter", false, "measure"),
            ("Tally::Base", false, "measure"),
            ("Tally::Meter", false, "suffix"),
            ("Tally::Meter", false, "prefix"),
            ("Tally::Counting", false, "count_up"),
            ("Tally::Counting", false, "step"),
        ] {
            assert!(reached.methods.contains(&method(owner, class_side, name)), "{owner} {name}: {reached:#?}");
        }
        assert!(!reached.methods.contains(&method("Tally::Counting", false, "count_down")), "{reached:#?}");
        assert!(!reached.classes.contains("Tally::Spare"), "{reached:#?}");
    }
}
