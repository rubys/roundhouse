//! One walk of a test body.
//!
//! Straight-line code, a gate (`if` / `case` / `&&`), and a block are
//! regions. A write in a region that does not contain the read is not
//! the value that read sees. A `lambda` / `proc` / `Proc.new` / `->`
//! runs later than the statement that stores it. An effect earlier in
//! the same expression runs before an `extend` there.
//!
//! Hoisting a module local and accepting an `extend` both read this
//! walk. They do not walk the tree again.

use std::collections::HashSet;

use super::{instance_extend, plain_new_class, var_name};
use crate::expr::{Expr, ExprNode, LValue, MatchPattern, Pattern};
use crate::ident::Symbol;

pub(super) struct Survey {
    events: Vec<Event>,
    sites: Vec<Site>,
    rebinds: HashSet<Symbol>,
}

struct Site {
    expr: Expr,
    is_stmt: bool,
    delayed: bool,
    preceded: bool,
    gated: bool,
}

pub(super) struct ExtendSite<'a> {
    pub expr: &'a Expr,
    pub is_stmt: bool,
    pub delayed: bool,
    pub preceded: bool,
    pub gated: bool,
    /// The class of the one straight-line `.new` that reaches this
    /// receiver. `None` when a second write, a pattern bind, or a write
    /// in another region does too.
    pub reaching_class: Option<String>,
}

enum Event {
    Write { name: Symbol, kind: WriteKind },
    Read { name: Symbol, extend_arg: bool },
    Site(usize),
    Child(Vec<Event>),
}

enum WriteKind {
    /// `name = <Class>.new(...)`.
    PlainNew(String),
    /// Any other plain `name = ...`.
    Plain,
    /// Op-assign, multi-assign, or a pattern bind.
    Other,
}

struct Builder {
    sites: Vec<Site>,
    rebinds: HashSet<Symbol>,
}

pub(super) fn survey(body: &Expr) -> Survey {
    let mut builder = Builder {
        sites: Vec::new(),
        rebinds: HashSet::new(),
    };
    let mut events = Vec::new();
    builder.statements(body, false, &mut events);
    Survey {
        events,
        sites: builder.sites,
        rebinds: builder.rebinds,
    }
}

/// An effect in this expression runs before an `extend` that is not
/// inside a block or a gate. Pulling that `extend` out front would
/// reverse the order.
pub(super) fn effect_before_extend(expr: &Expr) -> bool {
    let mut builder = Builder {
        sites: Vec::new(),
        rebinds: HashSet::new(),
    };
    let mut events = Vec::new();
    let mut effect = false;
    builder.survey(expr, true, false, false, false, &mut effect, &mut events);
    top_preceded(&events, &builder.sites)
}

fn top_preceded(events: &[Event], sites: &[Site]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::Site(id) if sites[*id].preceded))
}

impl Survey {
    pub(super) fn sole_local(&self, name: &Symbol) -> bool {
        if self.rebinds.contains(name) {
            return false;
        }
        let total = extend_reads(&self.events, name);
        let mut reads = 0u32;
        let mut extend_reads = 0u32;
        let mut home_writes = 0u32;
        let mut foreign = false;
        tally(
            &self.events,
            name,
            total,
            false,
            false,
            &mut reads,
            &mut extend_reads,
            &mut home_writes,
            &mut foreign,
        );
        !foreign && home_writes == 1 && reads > 0 && reads == extend_reads
    }

    pub(super) fn instance_extends(&self) -> Vec<ExtendSite<'_>> {
        self.sites
            .iter()
            .enumerate()
            .map(|(id, site)| {
                let reaching_class = var_name_of_receiver(&site.expr)
                    .and_then(|local| reaching(&self.events, id, &local));
                ExtendSite {
                    expr: &site.expr,
                    is_stmt: site.is_stmt,
                    delayed: site.delayed,
                    preceded: site.preceded,
                    gated: site.gated,
                    reaching_class,
                }
            })
            .collect()
    }
}

fn var_name_of_receiver(expr: &Expr) -> Option<Symbol> {
    instance_extend(expr).and_then(|(recv, _)| var_name(recv))
}

impl Builder {
    fn statements(&mut self, body: &Expr, delayed: bool, events: &mut Vec<Event>) {
        match &*body.node {
            ExprNode::Seq { exprs } => {
                for stmt in exprs {
                    let mut effect = false;
                    self.survey(stmt, true, delayed, false, false, &mut effect, events);
                }
            }
            _ => {
                let mut effect = false;
                self.survey(body, true, delayed, false, false, &mut effect, events);
            }
        }
    }

    fn survey(
        &mut self,
        expr: &Expr,
        is_stmt: bool,
        delayed: bool,
        gated: bool,
        extend_arg: bool,
        effect: &mut bool,
        events: &mut Vec<Event>,
    ) {
        match &*expr.node {
            ExprNode::Var { name, .. } => {
                events.push(Event::Read {
                    name: name.clone(),
                    extend_arg,
                });
            }
            ExprNode::Lambda { .. } => self.lambda(expr, true, events),
            ExprNode::Send { .. } => self.send(expr, is_stmt, delayed, gated, effect, events),
            ExprNode::Assign { target, value } => {
                self.survey(value, false, delayed, gated, false, effect, events);
                if let LValue::Var { name, .. } = target {
                    let kind = match plain_new_class(value) {
                        Some(class) => WriteKind::PlainNew(class),
                        None => WriteKind::Plain,
                    };
                    events.push(Event::Write {
                        name: name.clone(),
                        kind,
                    });
                }
                *effect = true;
            }
            ExprNode::OpAssign { target, op, value } => {
                let short = matches!(
                    op,
                    crate::expr::OpAssignOp::OrOr | crate::expr::OpAssignOp::AndAnd
                );
                if let LValue::Var { name, .. } = target {
                    let write = Event::Write {
                        name: name.clone(),
                        kind: WriteKind::Other,
                    };
                    if short {
                        *effect = true;
                        let mut child = Vec::new();
                        child.push(write);
                        let mut inner = false;
                        self.survey(value, false, delayed, true, false, &mut inner, &mut child);
                        events.push(Event::Child(child));
                        return;
                    }
                    events.push(write);
                }
                *effect = true;
                self.survey(value, false, delayed, gated, false, effect, events);
            }
            ExprNode::MultiAssign { targets, value } => {
                self.survey(value, false, delayed, gated, false, effect, events);
                for target in targets {
                    if let LValue::Var { name, .. } = target {
                        events.push(Event::Write {
                            name: name.clone(),
                            kind: WriteKind::Other,
                        });
                    }
                }
                *effect = true;
            }
            ExprNode::MatchRequired { value, pattern }
            | ExprNode::MatchPredicate { value, pattern } => {
                self.survey(value, false, delayed, gated, false, effect, events);
                push_binds(events, pattern);
                let mut inner = false;
                pattern.for_each_expr(&mut |child| {
                    self.survey(child, false, delayed, gated, false, &mut inner, events);
                });
                *effect = true;
            }
            _ if super::gates_control(expr) => {
                *effect = true;
                let mut child = Vec::new();
                self.gate(expr, delayed, &mut child);
                events.push(Event::Child(child));
            }
            ExprNode::Seq { exprs } => {
                for stmt in exprs {
                    self.survey(stmt, false, delayed, gated, false, effect, events);
                }
            }
            _ => {
                expr.node.for_each_child(&mut |child| {
                    self.survey(child, false, delayed, gated, false, effect, events);
                });
                if is_effect(expr) {
                    *effect = true;
                }
            }
        }
    }

    fn send(
        &mut self,
        expr: &Expr,
        is_stmt: bool,
        delayed: bool,
        gated: bool,
        effect: &mut bool,
        events: &mut Vec<Event>,
    ) {
        let ExprNode::Send {
            recv,
            method,
            args,
            block,
            ..
        } = &*expr.node
        else {
            return;
        };
        if instance_extend(expr).is_some() {
            let preceded = *effect;
            let id = self.sites.len();
            self.sites.push(Site {
                expr: expr.clone(),
                is_stmt,
                delayed,
                preceded,
                gated,
            });
            events.push(Event::Site(id));
            *effect = true;
            // The arguments are part of this call. A nested `extend`
            // is its own expression, so it does not inherit `preceded`.
            let mut inner = false;
            if let Some(recv) = recv {
                self.survey(recv, false, delayed, gated, false, &mut inner, events);
            }
            // Only `extend(mod)` reads its argument as the module local.
            // A second argument is a different call.
            let single = args.len() == 1;
            for arg in args {
                inner = false;
                self.survey(arg, false, delayed, gated, single, &mut inner, events);
            }
            if let Some(block) = block {
                self.lambda(block, false, events);
            }
            return;
        }
        if let Some(recv) = recv {
            self.survey(recv, false, delayed, gated, false, effect, events);
        }
        for arg in args {
            self.survey(arg, false, delayed, gated, false, effect, events);
        }
        if let Some(block) = block {
            let later = block_runs_later(recv.as_ref(), method.as_str());
            self.lambda(block, later, events);
        }
        *effect = true;
    }

    /// `delayed` is true for `->` and for `lambda` / `proc` / `Proc.new`.
    /// A call's block is false: it runs during the call.
    fn lambda(&mut self, expr: &Expr, delayed: bool, events: &mut Vec<Event>) {
        let ExprNode::Lambda {
            params,
            rest_param,
            extra_params,
            block_param,
            body,
            ..
        } = &*expr.node
        else {
            let mut effect = false;
            self.survey(expr, false, delayed, false, false, &mut effect, events);
            return;
        };
        for name in params {
            note_rebind(&mut self.rebinds, name);
        }
        if let Some(name) = rest_param {
            note_rebind(&mut self.rebinds, name);
        }
        if let Some(name) = block_param {
            note_rebind(&mut self.rebinds, name);
        }
        for param in extra_params {
            note_rebind(&mut self.rebinds, &param.name);
            if let Some(default) = &param.default {
                let mut effect = false;
                self.survey(default, false, delayed, false, false, &mut effect, events);
            }
        }
        let mut child = Vec::new();
        self.statements(body, delayed, &mut child);
        events.push(Event::Child(child));
    }

    fn gate(&mut self, expr: &Expr, delayed: bool, events: &mut Vec<Event>) {
        match &*expr.node {
            ExprNode::CaseMatch {
                scrutinee,
                arms,
                else_body,
            } => {
                let mut effect = false;
                self.survey(scrutinee, false, delayed, true, false, &mut effect, events);
                for arm in arms {
                    push_binds(events, &arm.pattern);
                    if let Some((_, guard)) = &arm.guard {
                        let mut effect = false;
                        self.survey(guard, false, delayed, true, false, &mut effect, events);
                    }
                    let mut effect = false;
                    self.survey(&arm.body, false, delayed, true, false, &mut effect, events);
                }
                if let Some(else_body) = else_body {
                    let mut effect = false;
                    self.survey(else_body, false, delayed, true, false, &mut effect, events);
                }
            }
            ExprNode::Case { scrutinee, arms } => {
                let mut effect = false;
                self.survey(scrutinee, false, delayed, true, false, &mut effect, events);
                for arm in arms {
                    push_pattern_binds(events, &arm.pattern);
                    if let Some(guard) = &arm.guard {
                        let mut effect = false;
                        self.survey(guard, false, delayed, true, false, &mut effect, events);
                    }
                    let mut effect = false;
                    self.survey(&arm.body, false, delayed, true, false, &mut effect, events);
                }
            }
            _ => expr.node.for_each_child(&mut |child| {
                let mut effect = false;
                self.survey(child, false, delayed, true, false, &mut effect, events);
            }),
        }
    }
}

fn note_rebind(rebinds: &mut HashSet<Symbol>, name: &Symbol) {
    if !name.as_str().is_empty() {
        rebinds.insert(name.clone());
    }
}

fn block_runs_later(recv: Option<&Expr>, method: &str) -> bool {
    let implicit = match recv {
        None => true,
        Some(recv) => matches!(&*recv.node, ExprNode::SelfRef),
    };
    if implicit && matches!(method, "lambda" | "proc") {
        return true;
    }
    method == "new" && recv.is_some_and(|recv| super::const_is(recv, "Proc"))
}

fn push_binds(events: &mut Vec<Event>, pattern: &MatchPattern) {
    let mut names = Vec::new();
    pattern.bound_names(&mut names);
    for name in names {
        note_write(events, name);
    }
}

fn push_pattern_binds(events: &mut Vec<Event>, pattern: &Pattern) {
    let mut names = Vec::new();
    pattern_names(pattern, &mut names);
    for name in names {
        note_write(events, name);
    }
}

fn pattern_names(pattern: &Pattern, out: &mut Vec<Symbol>) {
    match pattern {
        Pattern::Bind { name } => out.push(name.clone()),
        Pattern::Array { elems, .. } => {
            for elem in elems {
                pattern_names(elem, out);
            }
        }
        Pattern::Record { fields, .. } => {
            for (_, field) in fields {
                pattern_names(field, out);
            }
        }
        Pattern::Wildcard | Pattern::Lit { .. } | Pattern::Expr { .. } => {}
    }
}

fn note_write(events: &mut Vec<Event>, name: Symbol) {
    if name.as_str().is_empty() {
        return;
    }
    events.push(Event::Write {
        name,
        kind: WriteKind::Other,
    });
}

fn is_effect(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Send { .. }
            | ExprNode::Assign { .. }
            | ExprNode::OpAssign { .. }
            | ExprNode::MultiAssign { .. }
            | ExprNode::Yield { .. }
            | ExprNode::Raise { .. }
            | ExprNode::Super { .. }
            | ExprNode::Return { .. }
    )
}

fn extend_reads(events: &[Event], name: &Symbol) -> u32 {
    let mut n = 0u32;
    for event in events {
        match event {
            Event::Read {
                name: read,
                extend_arg: true,
            } if read == name => n += 1,
            Event::Child(child) => n += extend_reads(child, name),
            _ => {}
        }
    }
    n
}

fn tally(
    events: &[Event],
    name: &Symbol,
    total: u32,
    boundary: bool,
    inherited_foreign: bool,
    reads: &mut u32,
    matched: &mut u32,
    home_writes: &mut u32,
    foreign: &mut bool,
) {
    let covers = total > 0 && extend_reads(events, name) == total;
    let foreign_here = inherited_foreign || (boundary && !covers);
    for event in events {
        match event {
            Event::Read {
                name: read,
                extend_arg,
            } if read == name => {
                *reads += 1;
                if *extend_arg && !foreign_here {
                    *matched += 1;
                }
            }
            Event::Write {
                name: written,
                kind,
            } if written == name => {
                if foreign_here {
                    *foreign = true;
                } else {
                    match kind {
                        WriteKind::PlainNew(_) | WriteKind::Plain => *home_writes += 1,
                        WriteKind::Other => *home_writes += 2,
                    }
                }
            }
            Event::Child(child) => tally(
                child,
                name,
                total,
                true,
                foreign_here,
                reads,
                matched,
                home_writes,
                foreign,
            ),
            _ => {}
        }
    }
}

struct Reach {
    class: Option<String>,
    unclear: bool,
}

fn reaching(events: &[Event], site: usize, local: &Symbol) -> Option<String> {
    let mut reach = Reach {
        class: None,
        unclear: false,
    };
    find(events, site, local, &mut reach).flatten()
}

fn find(
    events: &[Event],
    site: usize,
    local: &Symbol,
    reach: &mut Reach,
) -> Option<Option<String>> {
    for event in events {
        match event {
            Event::Write { name, kind } if name == local => apply_write(reach, kind),
            Event::Site(id) if *id == site => {
                return Some(if reach.unclear {
                    None
                } else {
                    reach.class.clone()
                });
            }
            Event::Child(child) => {
                let mut inner = Reach {
                    class: reach.class.clone(),
                    unclear: reach.unclear,
                };
                if let Some(found) = find(child, site, local, &mut inner) {
                    return Some(found);
                }
                if writes_local(child, local) {
                    reach.unclear = true;
                    reach.class = None;
                }
            }
            _ => {}
        }
    }
    None
}

fn apply_write(reach: &mut Reach, kind: &WriteKind) {
    match kind {
        WriteKind::PlainNew(class) if !reach.unclear && reach.class.is_none() => {
            reach.class = Some(class.clone());
        }
        _ => {
            reach.unclear = true;
            reach.class = None;
        }
    }
}

fn writes_local(events: &[Event], local: &Symbol) -> bool {
    events.iter().any(|event| match event {
        Event::Write { name, .. } if name == local => true,
        Event::Child(child) => writes_local(child, local),
        _ => false,
    })
}
