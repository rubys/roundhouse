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
//! (`Rooms::Direct.find_or_create_for`) is the subclass's own; and
//! `find` is left alone because the arel pass inlines `<Model>.find` to
//! a primary-key SELECT — re-rooting it would move that work onto the
//! Relation path for no scoping gain a primary key can use.

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
    "group", "maximum", "minimum", "destroy_all", "delete_all", "update_all",
];

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

pub fn apply_sti_scope_lowering(app: &mut App) {
    let bases = sti_bases(app);
    if bases.is_empty() {
        return;
    }
    // Subclasses a `becomes!` call site actually names. Only these grow
    // the synthesized `becomes_from`: a class method nobody calls is
    // dead weight on every target, and on the strict ones it is dead
    // weight that still has to type-check.
    let mut recast: HashSet<ClassId> = HashSet::new();
    super::for_each_hook_body(app, &mut |e| rewrite(e, &bases, &mut recast));
    for view in &mut app.views {
        rewrite(&mut view.body, &bases, &mut recast);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &bases, &mut recast);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &bases, &mut recast);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &bases, &mut recast);
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
        let sub_ty = Ty::Class { id: lc.name.clone(), args: vec![] };
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
            name: Symbol::from(BECOMES_FROM),
            receiver: MethodReceiver::Class,
            params: vec![Param::positional(source.clone())],
            body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: body }),
            signature: Some(crate::lower::typing::fn_sig(
                vec![(source, Ty::Class { id: base.clone(), args: vec![] })],
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
    let mut out = HashMap::new();
    let model_named = |id: &ClassId| app.models.iter().find(|m| &m.name == id);
    for lc in &app.library_classes {
        let mut cursor = lc.parent.clone();
        let mut hops = 0;
        while let Some(parent) = cursor {
            if hops > 8 {
                break;
            }
            if let Some(base) = model_named(&parent) {
                let has_type = app
                    .schema
                    .tables
                    .get(&base.table.0)
                    .is_some_and(|t| {
                        t.columns.iter().any(|c| c.name.as_str() == INHERITANCE_COLUMN)
                    });
                if has_type {
                    out.insert(lc.name.clone(), base.name.clone());
                }
                break;
            }
            cursor = app
                .library_classes
                .iter()
                .find(|other| other.name == parent)
                .and_then(|other| other.parent.clone());
            hops += 1;
        }
    }
    out
}

fn rewrite(
    expr: &mut Expr,
    bases: &HashMap<ClassId, ClassId>,
    recast: &mut HashSet<ClassId>,
) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, bases, recast));
    if rewrite_becomes(expr, bases, recast) {
        return;
    }
    let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return };
    let constructing = STI_CONSTRUCTORS.contains(&method.as_str());
    if !constructing && !STI_RELATION_METHODS.contains(&method.as_str()) {
        return;
    }
    let ExprNode::Const { path } = &*r.node else { return };
    let named = ClassId(Symbol::from(
        path.iter()
            .map(|s| s.as_str().trim_start_matches("::"))
            .collect::<Vec<_>>()
            .join("::"),
    ));
    let Some(_) = bases.get(&named) else { return };
    if constructing {
        stamp_type(expr, &named);
        return;
    }
    let base = bases.get(&named).expect("checked above");
    let span = expr.span;
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
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Str { value: named.0.as_str().to_string() },
                            },
                        ),
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
    let ExprNode::Send { recv: Some(_), method, args, block, .. } = &*expr.node else {
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
    let source = recv.expect("checked above");
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
