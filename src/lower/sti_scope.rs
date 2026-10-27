//! `Rooms::Open.count` — an STI SUBCLASS standing at a query root.
//!
//! Rails' single-table inheritance gives a subclass its own default
//! scope: `Rooms::Open.count` counts the rooms whose `type` column says
//! `"Rooms::Open"`, not every row in `rooms`.
//!
//! An STI subclass is not ingested as a Model (it declares no table of
//! its own), so every one of those call sites resolved by plain Ruby
//! inheritance to the BASE class's method and answered the whole table.
//! campfire has seven rooms across three subclasses, so
//! `Rooms::Open.count` answered 7 where Rails answers 2 — a silently
//! wrong number, and the reason `user_test`'s membership-grant assertion
//! could pass while nothing was scoped: it compares that expression
//! against itself.
//!
//! The rewrite is a call-site one, into vocabulary every target already
//! speaks:
//!
//! ```text
//! Rooms::Open.pluck(:id)  ->  Room.where(type: "Rooms::Open").pluck(:id)
//! Rooms::Open.all         ->  Room.where(type: "Rooms::Open")
//! ```
//!
//! Synthesizing per-subclass class methods was the alternative and is
//! worse twice over: the surface is the whole Relation API, and each
//! method would have to land on every target.
//!
//! **`becomes!` is the one place a synthesized class method IS right.**
//! `room.becomes!(Rooms::Open)` recasts a loaded row to an STI sibling,
//! so unlike the relation surface it cannot be expressed by re-rooting
//! the call — the RESULT has to be an instance of the named subclass
//! (campfire's `Rooms::Open#grant_access_to_all_users` is a callback
//! that only exists on the sibling, and only fires if the object is
//! one). The target is always a literal constant at the call site, so
//! no runtime dispatch is needed: the call becomes
//! `Rooms::Open.becomes_from(room)`, and this pass pushes that class
//! method onto the subclass. Its return type is the subclass, which is
//! what keeps `@room = @room.becomes!(Rooms::Closed)` assignable on the
//! strict targets — a shared-runtime instance method could only have
//! returned `ActiveRecord::Base`.
//!
//! The COLUMN COPY is written out here too, one named column at a
//! time, and that placement was paid for: a shared-runtime
//! `Base#becomes_state_from(source)` looping over `source.attributes`
//! compiles on the ruby target and does not exist on Rust, where
//! `Base` is a plain struct and `attributes` lives on each model. The
//! generic loop needs the polymorphic dispatch back to the subclass
//! that the strict targets are built to avoid. Here the schema is
//! known at lowering time, so the loop unrolls into concrete `[]`/`[]=`
//! reads and writes and nothing needs dispatching.
//!
//! **Two halves.** The RELATION surface filters; the CONSTRUCTORS
//! (`new`/`create!`/…) STAMP, folding `type: "Rooms::Open"` into the
//! attribute hash — without it `Rooms::Open.create!` wrote a row
//! belonging to no subclass, which the read half above then correctly
//! could not see. A hand-written class method on the subclass
//! (`Rooms::Direct.find_or_create_for`) is the subclass's own.
//!
//! **A subclass's rows include its own subclasses'** (#1415, activerecord
//! 8.1.4): `Feature.count` with `SpecialFeature < Feature` counts both,
//! as Rails' `type_condition` filters on `type IN ('Feature',
//! 'SpecialFeature')`. A subclass with descendants is scoped by that
//! list, one without by its name, as before. `find` is scoped too: Rails
//! raises RecordNotFound for another type's id, and the relation's `find`
//! does, where the base's primary-key SELECT answered the row. The base
//! model's named scopes, called on a subclass, are scoped like the rest.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// The Relation entry points a subclass inherits and must scope. `all`
/// is handled separately (it IS the scope, so the call disappears).
const STI_RELATION_METHODS: &[&str] = &[
    "all", "count", "pluck", "ids", "where", "find_by", "find_by!", "first", "last", "exists?",
    "any?", "none?", "empty?", "order", "limit", "offset", "joins", "includes", "distinct",
    "group", "maximum", "minimum", "destroy_all", "delete_all", "update_all", "sum", "take",
    "pick", "select", "none", "find_each", "find",
];

/// Find-or-build on a subclass: Rails' `find_by(attrs) || create(attrs)`,
/// looked up in the subclass's whole scope (a deeper subclass's row is
/// found) and built as the subclass ([`seeding_lookup`]). An argument
/// that isn't a literal Hash of pure reads is filtered on the subclass's
/// own name instead, since `where` seeds a new record's `type` from an
/// equality only: there a deeper subclass's row isn't found.
const STI_SEEDING_METHODS: &[&str] = &["find_or_create_by", "find_or_create_by!", "find_or_initialize_by"];

/// Construction on a subclass, which has to STAMP the type column
/// rather than filter on it. `Rooms::Open.create!(name: …)` wrote a row
/// with an empty `type`, so it belonged to no subclass at all — and the
/// read half above made that visible, because a correctly-scoped
/// `Rooms::Open.count` then cannot see the row it just created.
const STI_CONSTRUCTORS: &[&str] = &["new", "create", "create!", "build"];

/// Rails' inheritance column. Not configurable here — `inheritance_column`
/// is a per-model override no corpus app writes, and guessing at one
/// would filter on a column that isn't there.
const INHERITANCE_COLUMN: &str = "type";

/// The synthesized recast entry point `becomes!` lowers to.
const BECOMES_FROM: &str = "becomes_from";

/// What the call-site rewrite needs of each STI subclass.
struct Sti {
    /// Subclass -> its base model.
    bases: HashMap<ClassId, ClassId>,
    /// Subclass -> the `type` names its rows carry: its own, then its
    /// descendants', sorted.
    names: HashMap<ClassId, Vec<String>>,
    /// Base model -> its named scopes.
    scopes: HashMap<ClassId, HashSet<String>>,
}

impl Sti {
    fn new(app: &App, bases: HashMap<ClassId, ClassId>) -> Self {
        let parent_of: HashMap<&ClassId, &ClassId> =
            app.library_classes.iter().filter_map(|lc| Some((&lc.name, lc.parent.as_ref()?))).collect();
        let mut names: HashMap<ClassId, Vec<String>> = HashMap::new();
        for sub in bases.keys() {
            // Every subclass whose parent chain reaches `sub`.
            let mut own: Vec<String> = bases
                .keys()
                .filter(|other| *other != sub)
                .filter(|other| {
                    let mut cursor = parent_of.get(other).copied();
                    let mut hops = 0;
                    while let Some(p) = cursor {
                        if p == sub {
                            return true;
                        }
                        hops += 1;
                        if hops > 8 {
                            break;
                        }
                        cursor = parent_of.get(p).copied();
                    }
                    false
                })
                .map(|c| c.0.as_str().to_string())
                .collect();
            own.sort();
            own.insert(0, sub.0.as_str().to_string());
            names.insert(sub.clone(), own);
        }
        let scopes = app
            .models
            .iter()
            .filter(|m| bases.values().any(|b| *b == m.name))
            .map(|m| (m.name.clone(), m.scopes().map(|s| s.name.as_str().to_string()).collect()))
            .collect();
        Sti { bases, names, scopes }
    }
}

pub fn apply_sti_scope_lowering(app: &mut App) {
    let bases = sti_bases(app);
    if bases.is_empty() {
        return;
    }
    let sti = Sti::new(app, bases.clone());
    // Subclasses a `becomes!` call site actually names. Only these grow
    // the synthesized `becomes_from`: a class method nobody calls is
    // dead weight on every target, and on the strict ones it is dead
    // weight that still has to type-check.
    let mut recast: HashSet<ClassId> = HashSet::new();
    // Inside a subclass's OWN class method the receiver is implicit:
    // campfire's `Rooms::Direct.find_for` writes `all.joins(:users)
    // .detect { … }`, and Rails scopes that `all` to the subclass. Left
    // bare, it resolved to the base's and searched EVERY room, so the
    // open room both users belong to matched "the direct room with
    // exactly these members" — starting a DM redirected into it and no
    // direct room was ever created (found on the Fly deploy). Naming the
    // subclass here hands the call to the rewrite below.
    for lc in &mut app.library_classes {
        if !bases.contains_key(&lc.name) {
            continue;
        }
        let sub = lc.name.clone();
        for m in &mut lc.methods {
            if matches!(m.receiver, MethodReceiver::Class) {
                name_implicit_receiver(&mut m.body, &sub);
            }
        }
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &sti, &mut recast));
    for view in &mut app.views {
        rewrite(&mut view.body, &sti, &mut recast);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &sti, &mut recast);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &sti, &mut recast);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &sti, &mut recast);
        }
    }
    push_becomes_from(app, &bases, &recast);
    stamp_sti_subclasses(app, &bases);
}

/// Record each base model's STI subclasses on the model itself
/// ([`Model::sti_subclass_names`]) — the fact `push_dom_prefix_method`
/// needs to make the base's `dom_prefix` a type-column dispatch, and
/// which only this pass derives (subclasses are library classes, not
/// Models, so the per-model synthesizer can't see them). Sorted, so
/// the generated `case` arms are deterministic.
fn stamp_sti_subclasses(app: &mut App, bases: &HashMap<ClassId, ClassId>) {
    let mut by_base: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
    for (subclass, base) in bases {
        by_base.entry(base.clone()).or_default().push(subclass.clone());
    }
    for (base, mut subs) in by_base {
        subs.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        if let Some(model) = app.models.iter_mut().find(|m| m.name == base) {
            model.sti_subclass_names = subs;
        }
    }
}

/// Give every recast target a `self.becomes_from(source)`:
///
/// ```text
/// def self.becomes_from(source)
///   record = Rooms::Open.new
///   record[:created_at] = source[:created_at]
///   record[:creator_id] = source[:creator_id]
///   record[:name] = source[:name]
///   record[:type] = "Rooms::Open"          # the stamp
///   record[:updated_at] = source[:updated_at]
///   record.id = source.id
///   record.mark_persisted! if source.persisted?
///   record
/// end
/// ```
///
/// Columns come from the BASE model's table — an STI subclass declares
/// none of its own — and are copied through `[]`/`[]=` rather than the
/// named accessors, because the column-to-slot mapping is the model's
/// own (a timestamp column reaches `@created_at_raw`) and the indexer
/// is where each model already writes that mapping down.
fn push_becomes_from(app: &mut App, bases: &HashMap<ClassId, ClassId>, recast: &HashSet<ClassId>) {
    // Column list per base model, read before the mutable borrow below.
    let columns: HashMap<ClassId, Vec<Symbol>> = app
        .models
        .iter()
        .map(|m| {
            let cols = app
                .schema
                .tables
                .get(&m.table.0)
                .map(|t| t.columns.iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default();
            (m.name.clone(), cols)
        })
        .collect();

    for lc in &mut app.library_classes {
        if !recast.contains(&lc.name) {
            continue;
        }
        let Some(base) = bases.get(&lc.name) else { continue };
        if lc.methods.iter().any(|m| m.name.as_str() == BECOMES_FROM) {
            continue;
        }
        let source = Symbol::from("source");
        let record = Symbol::from("record");
        let sub_ty = Ty::Class { id: lc.name.clone(), args: vec![].into() };
        let mut body: Vec<Expr> = vec![Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: record.clone() },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(class_const(&lc.name)),
                        method: Symbol::from("new"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    },
                ),
            },
        )];
        // `record[:col] = source[:col]` per column — the indexer, not
        // the named accessor, because the column-to-slot mapping is the
        // model's own (a timestamp column reaches `@created_at_raw`)
        // and `[]`/`[]=` are where each model already writes it down.
        for col in columns.get(base).map(|v| v.as_slice()).unwrap_or_default() {
            // The primary key rides its own accessor below — `id` is on
            // Base whether or not the schema lists it as a column, so
            // writing it here as well would write it twice.
            if col.as_str() == "id" {
                continue;
            }
            let value = if col.as_str() == INHERITANCE_COLUMN {
                // The stamp: a recast row belongs to the new subclass.
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Str { value: lc.name.0.as_str().to_string() },
                    },
                )
            } else {
                index_read(var_ref(&source), col)
            };
            body.push(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(&record)),
                    method: Symbol::from("[]="),
                    args: vec![sym_lit(col), value],
                    block: None,
                    parenthesized: true,
                },
            ));
        }
        // The id is the whole point of a recast — the sibling IS this
        // row — and `attr_accessor :id` on Base is the one writer every
        // model has, schema listing or not.
        body.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(&record)),
                method: Symbol::from("id="),
                args: vec![no_arg_send(var_ref(&source), "id")],
                block: None,
                parenthesized: true,
            },
        ));
        // Carry the persisted flag: a recast row is the SAME row, so the
        // save after it must UPDATE. Without this the next save inserts
        // a duplicate.
        body.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: no_arg_send(var_ref(&source), "persisted?"),
                then_branch: no_arg_send(var_ref(&record), "mark_persisted!"),
                else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
        ));
        body.push(var_ref(&record));
        lc.methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from(BECOMES_FROM),
            receiver: MethodReceiver::Class,
            params: vec![Param::positional(source.clone())],
            body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: body }),
            signature: Some(crate::lower::typing::fn_sig(
                vec![(source, Ty::Class { id: base.clone(), args: vec![].into() })],
                sub_ty,
            )),
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(lc.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// `all` / `where(…)` / … with no receiver (or `self`) in a subclass's
/// class method → the same call on the subclass constant. Only the
/// ROOT of a chain has no receiver, so the walk reaches it through the
/// receivers above; block bodies are walked too (a nested query in a
/// `detect` block is still the class's own).
fn name_implicit_receiver(expr: &mut Expr, sub: &ClassId) {
    expr.node.for_each_child_mut(&mut |c| name_implicit_receiver(c, sub));
    let ExprNode::Send { recv, method, .. } = &mut *expr.node else { return };
    if !STI_RELATION_METHODS.contains(&method.as_str()) {
        return;
    }
    let implicit = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if implicit {
        *recv = Some(class_const(sub));
    }
}

fn class_const(id: &ClassId) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: id.0.as_str().split("::").map(Symbol::from).collect() },
    )
}

fn var_ref(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

fn sym_lit(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name.clone() } })
}

fn index_read(recv: Expr, col: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("[]"),
            args: vec![sym_lit(col)],
            block: None,
            parenthesized: true,
        },
    )
}

fn no_arg_send(recv: Expr, method: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// STI subclass → its base model. A subclass is a library class whose
/// `parent` chain reaches a Model, and that Model's table has to carry
/// the inheritance column — without it there is nothing to filter on
/// and the class is plain Ruby inheritance, not STI.
/// Public because the Ruby-family hydration pass
/// (`emit::ruby::library::apply_sti_hydration`) has to ask the SAME
/// question this file already answers — which classes are STI
/// subclasses of which base — rather than keeping a second copy of the
/// parent walk and the inheritance-column check that would drift from
/// this one.
pub(crate) fn sti_bases(app: &App) -> HashMap<ClassId, ClassId> {
    model_subclasses(app).0
}

/// Every library class whose `parent` chain reaches a Model with a
/// table, split on that table's inheritance column: STI subclasses
/// (with one) and plain subclasses (without one) → their base model.
fn model_subclasses(app: &App) -> (HashMap<ClassId, ClassId>, HashMap<ClassId, ClassId>) {
    let mut typed = HashMap::new();
    let mut untyped = HashMap::new();
    // Name -> the FIRST model / library class of that name, the same
    // element the linear `find`s this replaced answered. Indexed once:
    // a scan per parent hop is |classes|^2 on a large app.
    let mut models_by_name: HashMap<&ClassId, &crate::dialect::Model> = HashMap::new();
    for m in &app.models {
        models_by_name.entry(&m.name).or_insert(m);
    }
    let mut lc_parent: HashMap<&ClassId, &Option<ClassId>> = HashMap::new();
    for other in &app.library_classes {
        lc_parent.entry(&other.name).or_insert(&other.parent);
    }
    let model_named = |id: &ClassId| models_by_name.get(id).copied();
    for lc in &app.library_classes {
        let mut cursor = lc.parent.clone();
        let mut hops = 0;
        while let Some(parent) = cursor {
            if hops > 8 {
                break;
            }
            if let Some(base) = model_named(&parent) {
                // An abstract base has no table, so its subclasses are
                // neither.
                if let Some(table) = app.schema.tables.get(&base.table.0) {
                    if table.columns.iter().any(|c| c.name.as_str() == INHERITANCE_COLUMN) {
                        typed.insert(lc.name.clone(), base.name.clone());
                    } else {
                        untyped.insert(lc.name.clone(), base.name.clone());
                    }
                }
                break;
            }
            cursor = lc_parent.get(&parent).and_then(|p| (*p).clone());
            hops += 1;
        }
    }
    (typed, untyped)
}

/// The finders that load a subclass's records, or a relation of them.
/// Value answers (`count`, `pluck`, `sum`, …) and the constructors are
/// already Rails' without a `type` column.
const LOADING_METHODS: &[&str] = &[
    "all", "where", "find", "find_by", "find_by!", "first", "first!", "last", "last!", "take",
    "take!", "sole", "find_sole_by", "find_each", "find_in_batches", "in_batches", "order",
    "reorder", "limit", "offset", "joins", "left_joins", "left_outer_joins", "includes",
    "preload", "eager_load", "distinct", "select", "group", "having", "unscoped",
    "readonly", "lock", "find_or_create_by", "find_or_create_by!", "find_or_initialize_by",
    "create_or_find_by", "create_or_find_by!",
];

/// Relation-preserving sends: an aggregate on this chain still answers a
/// value and loads no subclass row (`Feature.where(…).count`). Eager
/// finders (`find`, `first`, `find_by`, …) are not here — they load
/// records even when an aggregate is written on the result
/// (`Feature.find([1, 2]).count`), so the subclass warning still fires.
const RELATION_CHAIN_METHODS: &[&str] = &[
    "all", "where", "order", "reorder", "limit", "offset", "joins", "left_joins",
    "left_outer_joins", "includes", "preload", "eager_load", "distinct", "select", "group",
    "having", "unscoped", "readonly", "lock",
];

/// Aggregates that answer a value from a relation, not its records: the
/// relation they're called on loads nothing (`Feature.where(…).count`).
const VALUE_METHODS: &[&str] = &[
    "count", "pluck", "pick", "ids", "sum", "minimum", "maximum", "average", "calculate",
    "exists?", "any?", "none?", "empty?", "many?", "size", "delete_all", "update_all",
];

/// A subclass of a concrete model whose table has no `type` column
/// (#1415). That isn't STI: activerecord 8.1.4 shares the table, filters
/// nothing, and loads every row a subclass's finder answers as the
/// subclass (`Feature.first.class` is `Feature`, `Feature.count` counts
/// every row). Here the finder is the base's, inherited, and its rows
/// are the base's class: `Feature.first.class` is `Article`, so a
/// method only the subclass defines raises NoMethodError on them.
/// Counts and constructors already match. Each call site with the
/// subclass as its constant receiver and a loading finder, or one of
/// the base's named scopes, is a warning: the call is still emitted.
/// Not one that's the relation of an aggregate (`Feature.where(…).count`
/// answers a value), and the bodies walked are the ones the STI
/// lowering rewrites, tests included.
pub(crate) fn untyped_subclass_diagnostics(app: &App) -> Vec<crate::diagnostic::Diagnostic> {
    let (_, untyped) = model_subclasses(app);
    if untyped.is_empty() {
        return Vec::new();
    }
    let scopes: HashMap<&ClassId, HashSet<&str>> = app
        .models
        .iter()
        .map(|m| {
            let names = m
                .body
                .iter()
                .filter_map(|item| match item {
                    crate::dialect::ModelBodyItem::Scope { scope, .. } => Some(scope.name.as_str()),
                    _ => None,
                })
                .collect();
            (&m.name, names)
        })
        .collect();
    // `valued`: `e` is the receiver chain of an aggregate, so its sends
    // load nothing. Their arguments and blocks are walked as usual.
    fn walk(
        e: &Expr,
        valued: bool,
        untyped: &HashMap<ClassId, ClassId>,
        scopes: &HashMap<&ClassId, HashSet<&str>>,
        sites: &mut Vec<(Span, ClassId, ClassId, Symbol)>,
    ) {
        let ExprNode::Send { recv, method, args, block, .. } = &*e.node else {
            e.node.for_each_child(&mut |c| walk(c, false, untyped, scopes, sites));
            return;
        };
        if let Some(recv) = recv {
            if let ExprNode::Const { path } = &*recv.node {
                let sub = const_class_id(path);
                if let Some(base) = untyped.get(&sub) {
                    let named_scope = scopes.get(base).is_some_and(|names| names.contains(method.as_str()));
                    let loads = LOADING_METHODS.contains(&method.as_str()) || named_scope;
                    // An aggregate suppresses only relation-preserving
                    // sends (and named scopes). Eager finders still load
                    // subclass rows when written as `Feature.find(…).count`.
                    let relation_chain =
                        RELATION_CHAIN_METHODS.contains(&method.as_str()) || named_scope;
                    if loads && !(valued && relation_chain) {
                        sites.push((e.span, sub, base.clone(), method.clone()));
                    }
                }
            }
            let chain = valued || VALUE_METHODS.contains(&method.as_str());
            walk(recv, chain, untyped, scopes, sites);
        }
        for arg in args {
            walk(arg, false, untyped, scopes, sites);
        }
        if let Some(block) = block {
            walk(block, false, untyped, scopes, sites);
        }
    }
    let mut sites: Vec<(Span, ClassId, ClassId, Symbol)> = Vec::new();
    let mut visit = |body: &Expr| walk(body, false, &untyped, &scopes, &mut sites);
    // The bodies `apply_sti_scope_lowering` rewrites.
    super::for_each_hook_body_ref(app, &mut visit);
    for view in &app.views {
        visit(&view.body);
    }
    for tm in &app.test_modules {
        if let Some(setup) = &tm.setup {
            visit(setup);
        }
        for t in &tm.tests {
            visit(&t.body);
        }
        for m in &tm.helpers {
            visit(&m.body);
        }
    }
    sites
        .into_iter()
        .map(|(span, sub, base, method)| {
            let (sub, base) = (sub.0.as_str(), base.0.as_str());
            let mut diagnostic = crate::diagnostic::Diagnostic::unsupported(
                span,
                None,
                format!("`{sub}.{}` on a subclass of a model whose table has no `type` column", method.as_str()),
                format!("Rails loads its records as `{sub}`; this build loads them as `{base}`, whose finder it inherits"),
            );
            diagnostic.severity = crate::diagnostic::Severity::Warning;
            let file = crate::ide::source(app, span.file).map(|s| s.path.clone()).unwrap_or_default();
            crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                file,
                message: diagnostic.message.clone(),
            });
            diagnostic
        })
        .collect()
}

fn rewrite(
    expr: &mut Expr,
    sti: &Sti,
    recast: &mut HashSet<ClassId>,
) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, sti, recast));
    let bases = &sti.bases;
    if rewrite_becomes(expr, bases, recast) {
        return;
    }
    let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return };
    let constructing = STI_CONSTRUCTORS.contains(&method.as_str());
    let seeding = STI_SEEDING_METHODS.contains(&method.as_str());
    let method_name = method.as_str().to_string();
    let ExprNode::Const { path } = &*r.node else { return };
    let named = ClassId(Symbol::from(
        path.iter()
            .map(|s| s.as_str().trim_start_matches("::"))
            .collect::<Vec<_>>()
            .join("::"),
    ));
    let Some(base) = bases.get(&named) else { return };
    let base_scope = sti.scopes.get(base).is_some_and(|s| s.contains(&method_name));
    if !constructing && !seeding && !base_scope && !STI_RELATION_METHODS.contains(&method_name.as_str()) {
        return;
    }
    if constructing {
        stamp_type(expr, &named);
        return;
    }
    let span = expr.span;
    let str_lit = |value: &str| Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } });
    let type_names = sti.names.get(&named).map(Vec::as_slice).unwrap_or_default();
    let lookup_value = if type_names.len() <= 1 {
        str_lit(named.0.as_str())
    } else {
        Expr::new(
            span,
            ExprNode::Array { elements: type_names.iter().map(|n| str_lit(n)).collect(), style: Default::default() },
        )
    };
    // A find-or-build with a literal Hash of pure reads is Rails' own
    // definition, `find_by(attrs) || create(attrs)`: the lookup is the
    // subclass's whole scope, descendants included, and the record it
    // builds is the subclass, stamped with its name. Anything else is
    // filtered on the subclass's own name, so `where` seeds its `type`.
    if seeding {
        if let Some(found) = seeding_lookup(expr, base, &named, lookup_value.clone()) {
            *expr = found;
            return;
        }
    }
    let type_value = if seeding { str_lit(named.0.as_str()) } else { lookup_value };
    let scoped = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: base.0.as_str().split("::").map(Symbol::from).collect(),
                },
            )),
            method: Symbol::from("where"),
            args: vec![Expr::new(
                span,
                ExprNode::Hash {
                    entries: vec![(
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Sym { value: Symbol::from(INHERITANCE_COLUMN) },
                            },
                        ),
                        type_value,
                    )],
                    kwargs: true,
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    // `.all` IS the scope, so the call disappears into it. Everything
    // else rides the scoped relation.
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { method, args, block, parenthesized, .. } = node else { unreachable!() };
    if method.as_str() == "all" && args.is_empty() && block.is_none() {
        *expr = scoped;
        return;
    }
    *expr = Expr::new(
        span,
        ExprNode::Send { recv: Some(scoped), method, args, block, parenthesized },
    );
}

/// `Feature.find_or_create_by(title: "s")` ->
/// `Article.where(type: ["Feature", "SpecialFeature"]).find_by(title: "s")
/// || Feature.create(title: "s", type: "Feature")`, and `create!` / `new`
/// for `find_or_create_by!` / `find_or_initialize_by`. The block, if any,
/// goes to the constructor, as Rails passes it to `create`. None unless
/// the one argument is a literal Hash of pure reads: it is evaluated
/// twice.
fn seeding_lookup(expr: &Expr, base: &ClassId, sub: &ClassId, lookup_value: Expr) -> Option<Expr> {
    let ExprNode::Send { method, args, block, parenthesized, .. } = &*expr.node else { return None };
    let [attrs] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, .. } = &*attrs.node else { return None };
    if !entries.iter().all(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } }) && super::case_lambda::is_pure_read(v)
    }) {
        return None;
    }
    let constructor = match method.as_str() {
        "find_or_create_by" => "create",
        "find_or_create_by!" => "create!",
        _ => "new",
    };
    let span = expr.span;
    let send = |recv: Expr, method: &str, args: Vec<Expr>, block: Option<Expr>| {
        Expr::new(
            span,
            ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block, parenthesized: *parenthesized },
        )
    };
    let scope = send(
        class_const(base),
        "where",
        vec![Expr::new(
            span,
            ExprNode::Hash {
                entries: vec![(
                    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(INHERITANCE_COLUMN) } }),
                    lookup_value,
                )],
                kwargs: true,
            },
        )],
        None,
    );
    let found = send(scope, "find_by", vec![attrs.clone()], None);
    let mut built = send(class_const(sub), constructor, vec![attrs.clone()], block.clone());
    stamp_type(&mut built, sub);
    Some(Expr::new(
        span,
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, surface: Default::default(), left: found, right: built },
    ))
}

/// `room.becomes!(Rooms::Open)` -> `Rooms::Open.becomes_from(room)`.
///
/// Only `becomes!` — Rails' non-bang `becomes` leaves the inheritance
/// column alone, which is a different operation, and no corpus app
/// writes it. The receiver moves into an argument position, so it is
/// evaluated exactly once either way and any expression is safe there.
///
/// Returns true when it rewrote, so the caller skips the relation/
/// constructor paths below (they key off a Const RECEIVER; here the
/// const is the ARGUMENT).
fn rewrite_becomes(
    expr: &mut Expr,
    bases: &HashMap<ClassId, ClassId>,
    recast: &mut HashSet<ClassId>,
) -> bool {
    let ExprNode::Send { method, args, block, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "becomes!" || args.len() != 1 || block.is_some() {
        return false;
    }
    let ExprNode::Const { path } = &*args[0].node else { return false };
    let named = const_class_id(path);
    if !bases.contains_key(&named) {
        return false;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, .. } = node else { unreachable!() };
    // Implicit self — campfire's `Room#destroy_later` writes
    // `becomes!(Rooms::Closed)` on the record itself.
    let source = recv.unwrap_or_else(|| Expr::new(span, ExprNode::SelfRef));
    recast.insert(named.clone());
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(class_const(&named)),
            method: Symbol::from(BECOMES_FROM),
            args: vec![source],
            block: None,
            parenthesized: true,
        },
    );
    true
}

/// A `Const { path }` as the fully-qualified class name it names.
/// Leading `::` is trimmed per segment — the ingest keeps it on an
/// explicitly-rooted constant.
fn const_class_id(path: &[Symbol]) -> ClassId {
    ClassId(Symbol::from(
        path.iter()
            .map(|s| s.as_str().trim_start_matches("::"))
            .collect::<Vec<_>>()
            .join("::"),
    ))
}

/// Fold `type: "<Sub>"` into a constructor's attribute hash.
///
/// The RECEIVER stays the subclass: Ruby's inheritance constructs a
/// `Rooms::Open`, which is what campfire's `Room#open?` (`is_a?
/// (Rooms::Open)`) asks about — rerooting on the base would answer that
/// wrongly while fixing the column. Only the column is missing.
///
/// An existing `type:` wins; an argument that is not a literal Hash
/// (a params object, a variable) is left alone, because there is no
/// literal to fold into and building a `merge` here would hand the
/// constructor a shape it does not take.
fn stamp_type(expr: &mut Expr, sub: &ClassId) {
    let span = expr.span;
    let ExprNode::Send { args, .. } = &mut *expr.node else { return };
    let Some(attrs) = args.last_mut() else { return };
    let ExprNode::Hash { entries, .. } = &mut *attrs.node else { return };
    let already = entries.iter().any(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == INHERITANCE_COLUMN)
    });
    if already {
        return;
    }
    entries.push((
        Expr::new(
            span,
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(INHERITANCE_COLUMN) } },
        ),
        Expr::new(
            span,
            ExprNode::Lit { value: Literal::Str { value: sub.0.as_str().to_string() } },
        ),
    ));
}
