//! Schema-driven methods: attr accessors, table_name, schema_columns,
//! instantiate, initialize, attributes, [], []=, update.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model, Param};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::pluralize_snake;
use crate::schema::{Column, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::row::row_class_id;
use super::{
    class_const, fn_sig, is_id_column, lit_int, lit_str, lit_sym, nil_lit, self_ref, seq,
    ty_of_column, var_ref, with_ty,
};

pub(super) fn push_schema_methods(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    table: &Table,
    permitted_fields: Option<&[Symbol]>,
) {
    let owner = &model.name;

    // Per-column getter+setter for every column INCLUDING id.
    // Although ApplicationRecord declares `id`/`id=` in its baseline
    // (so the typer's dispatch resolved them either way), per-target
    // emitters need a concrete declaration on the subclass to emit a
    // typed field — TS won't infer `id: number` on Article from a
    // baseline registration alone. Tagging as AttributeReader/Writer
    // (via synth_attr_reader/writer) lets the walker emit `id: number`
    // as a field declaration. Spinel-blog's article.rb omits id from
    // attr_accessor because the runtime mixes it in via `class << self`,
    // but that's a Spinel-runtime convention; the universal IR
    // declares per-class.
    for col in &table.columns {
        methods.push(synth_attr_reader(owner, col));
        methods.push(synth_attr_writer(owner, col));
    }

    // def self.table_name
    methods.push(MethodDef {
        name: Symbol::from("table_name"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: lit_str(pluralize_snake(model.name.0.as_str())),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    });

    // def self.schema_columns
    let column_array = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: table
                    .columns
                    .iter()
                    .map(|c| lit_sym(c.name.clone()))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        ),
        Ty::Array { elem: Box::new(Ty::Sym) },
    );
    methods.push(MethodDef {
        name: Symbol::from("schema_columns"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: column_array,
        signature: Some(fn_sig(vec![], Ty::Array { elem: Box::new(Ty::Sym) })),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    });

    // def self.instantiate(row); instance = from_row(<Model>Row.from_raw(row)); instance.mark_persisted!; instance; end
    //
    // The adapter shim returns Hash[Symbol, untyped]; the framework Ruby
    // narrows it once via `<Model>Row.from_raw(row)` and then constructs
    // the model via `<Model>.from_row(typed_row)`. The Hash-shaped
    // boundary stops at `from_raw`; everything downstream is typed.
    methods.push(synth_instantiate(owner));

    // def self.from_row(row); instance = new; instance.<col> = row.<col>; ...; instance; end
    //
    // Per-target emitters get a typed factory: input is `<Model>Row`
    // (typed slots from the schema), output is the persisted model. No
    // Hash flowing through. Pattern (b) from the handoff: separate
    // class-method factories rather than overloaded initialize.
    methods.push(synth_from_row(owner, table));

    // def assign_from_row(row); self.<col> = row[:<col>]; ...; end
    //
    // Instance-level reload helper. ActiveRecord::Base#reload re-fetches
    // the row via the adapter (returns Hash[Symbol, untyped]) and
    // dispatches to `assign_from_row(row)` to mutate the existing
    // instance in place. Indexing via `row[:col]` rather than typed
    // accessors so the path stays Hash-shaped — `from_row` already
    // covers the typed-Row construction case.
    methods.push(synth_assign_from_row(owner, table));

    // def initialize(attrs = {}); super(); per-column self.col = attrs[:col] [|| 0 for id]; end
    methods.push(synth_initialize(owner, table));

    // def attributes; { col: @col, ... } excluding id; end
    methods.push(synth_attributes(owner, table));

    // def [](name); case name; when :col then @col; ...; end; end
    methods.push(synth_index_read(owner, table));

    // def []=(name, value); case name; when :col then @col = value; ...; end; end
    methods.push(synth_index_write(owner, table));

    // def update(<arg>); per-permitted-field setter; save; end
    //
    // When a controller permits this model's resource, `update` takes the
    // typed `<Resource>Params` and assigns each permitted field via
    // `attr_writer` (no `.key?` check needed — `*Params` always carries
    // every permitted field). When no spec applies (rare; model not
    // exposed by any controller), falls back to the Hash-shaped variant
    // for backward compatibility.
    methods.push(match permitted_fields {
        Some(fields) => synth_update_typed(owner, fields),
        None => synth_update(owner, table),
    });
}

fn synth_attr_reader(owner: &ClassId, col: &Column) -> MethodDef {
    let col_ty = ty_of_column(&col.col_type);
    let body = with_ty(
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.name.clone() }),
        col_ty.clone(),
    );
    MethodDef {
        name: col.name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], col_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
    }
}

fn synth_attr_writer(owner: &ClassId, col: &Column) -> MethodDef {
    let value_param = Symbol::from("value");
    let col_ty = ty_of_column(&col.col_type);
    let rhs = with_ty(var_ref(value_param.clone()), col_ty.clone());
    // Assign expression evaluates to the RHS in Ruby; same in TS.
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: col.name.clone() },
                value: rhs,
            },
        ),
        col_ty.clone(),
    );
    MethodDef {
        name: Symbol::from(format!("{}=", col.name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param.clone())],
        body,
        signature: Some(fn_sig(vec![(value_param, col_ty.clone())], col_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeWriter,
    }
}

fn synth_instantiate(owner: &ClassId) -> MethodDef {
    let row = Symbol::from("row");
    let instance = Symbol::from("instance");
    let row_class = row_class_id(owner);

    // <Model>Row.from_raw(row) — narrow the Hash[Symbol, untyped] to the
    // typed row holder once. Everything downstream sees typed slots.
    let from_raw_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&row_class)),
            method: Symbol::from("from_raw"),
            args: vec![var_ref(row.clone())],
            block: None,
            parenthesized: true,
        },
    );

    // <Model>.from_row(<typed_row>) — typed factory.
    let from_row_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("from_row"),
            args: vec![from_raw_call],
            block: None,
            parenthesized: true,
        },
    );

    let body = seq(vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: instance.clone() },
                value: from_row_call,
            },
        ),
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from("mark_persisted!"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        ),
        var_ref(instance),
    ]);

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    // The adapter returns Hash[Symbol, untyped]; that's the public
    // signature of `instantiate`. Internal narrowing happens in the body.
    let row_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    MethodDef {
        name: Symbol::from("instantiate"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(row.clone())],
        body,
        signature: Some(fn_sig(vec![(row, row_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// `def self.from_params(p); instance = new; instance.<f> = p.<f>; ...; instance; end`
///
/// Typed counterpart to `from_row` for the controller-params boundary.
/// `fields` is the `permit(...)` list: only those columns are assigned
/// (id / timestamps / FKs aren't user-controllable). Other columns
/// stay at the defaults set by `initialize` from the empty Hash.
pub(super) fn push_from_params_method(
    methods: &mut Vec<MethodDef>,
    model: &crate::dialect::Model,
    fields: &[Symbol],
) {
    let owner = &model.name;
    let p = Symbol::from("p");
    let instance = Symbol::from("instance");
    let resource = Symbol::from(crate::naming::snake_case(owner.0.as_str()));
    let params_class_id = ClassId(Symbol::from(format!(
        "{}Params",
        crate::naming::camelize(resource.as_str())
    )));

    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );

    let mut stmts: Vec<Expr> = Vec::new();
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));

    for field in fields {
        let p_field = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(p.clone())),
                method: field.clone(),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![p_field],
                block: None,
                parenthesized: false,
            },
        ));
    }

    stmts.push(var_ref(instance));

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let params_ty = Ty::Class { id: params_class_id, args: vec![] };
    methods.push(MethodDef {
        name: Symbol::from("from_params"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(p.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(p, params_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    });
}

/// `def self.from_row(row); instance = new; instance.col = row.col; ...; instance; end`
///
/// The typed counterpart to the (still-existing) Hash-receiving
/// `initialize`. Takes a `<Model>Row` (typed slots) and produces a
/// fresh model instance with each column copied through. The model's
/// `initialize` runs as bare `new` here — field defaults from
/// `synth_initialize`'s empty-Hash branch (since attrs is `{}`).
fn synth_from_row(owner: &ClassId, table: &Table) -> MethodDef {
    let row = Symbol::from("row");
    let instance = Symbol::from("instance");
    let row_class = row_class_id(owner);

    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );

    let mut stmts: Vec<Expr> = Vec::new();
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));

    for col in &table.columns {
        // row.<col> — typed accessor on <Model>Row.
        let row_field = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(row.clone())),
                method: col.name.clone(),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![row_field],
                block: None,
                parenthesized: false,
            },
        ));
    }

    stmts.push(var_ref(instance));

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let row_ty = Ty::Class { id: row_class, args: vec![] };
    MethodDef {
        name: Symbol::from("from_row"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(row.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(row, row_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// `def assign_from_row(row); self.<col> = row[:<col>]; ...; end`
/// — mutates `self`, used by `ActiveRecord::Base#reload` after the
/// adapter re-fetches the row as a `Hash[Symbol, untyped]`. The Hash
/// stays Hash-shaped (no typed Row narrowing) since reload only
/// touches the existing instance's slots.
fn synth_assign_from_row(owner: &ClassId, table: &Table) -> MethodDef {
    let row = Symbol::from("row");
    let row_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };

    let mut stmts: Vec<Expr> = Vec::new();
    for col in &table.columns {
        // row[:<col>] — Hash index lookup keyed on the column symbol.
        let key = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Sym { value: col.name.clone() } },
            ),
            Ty::Sym,
        );
        let lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(row.clone())),
                method: Symbol::from("[]"),
                args: vec![key],
                block: None,
                parenthesized: false,
            },
        );
        // self.<col>= = row[:<col>]
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![lookup],
                block: None,
                parenthesized: false,
            },
        ));
    }

    MethodDef {
        name: Symbol::from("assign_from_row"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(row.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(row, row_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

fn synth_initialize(owner: &ClassId, table: &Table) -> MethodDef {
    let attrs = Symbol::from("attrs");

    let mut stmts: Vec<Expr> = Vec::new();
    // super() — calls ActiveRecord::Base#initialize.
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Super { args: Some(Vec::new()) },
    ));

    for col in &table.columns {
        let lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(attrs.clone())),
                method: Symbol::from("[]"),
                args: vec![lit_sym(col.name.clone())],
                block: None,
                parenthesized: false,
            },
        );
        // ID-shaped columns get `|| 0` defaults; spinel-blog's
        // article.rb defaults `id` and comment.rb defaults
        // `article_id` the same way. The "0 means unset" sentinel
        // matches the FK-resolution conventions used by belongs_to.
        let value = if is_id_column(&col.name) {
            Expr::new(
                Span::synthetic(),
                ExprNode::BoolOp {
                    op: crate::expr::BoolOpKind::Or,
                    surface: crate::expr::BoolOpSurface::Symbol,
                    left: lookup,
                    right: lit_int(0),
                },
            )
        } else {
            lookup
        };

        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        ));
    }

    // Spinel-blog's `def initialize(attrs = {})` — empty hash default
    // lets `Article.new` (no args) succeed, which the controller's
    // `new_action` relies on AND the synthesized `from_params` /
    // `from_row` factories rely on. Mark the signature param as
    // Optional so per-target emitters (TS specifically) emit
    // `attrs?: ...` and zero-arg `new Article()` from the factories
    // type-checks.
    let attrs_default = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries: Vec::new(), braced: true },
    );
    let attrs_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let signature = Ty::Fn {
        params: vec![crate::ty::Param {
            name: attrs.clone(),
            ty: attrs_ty,
            kind: crate::ty::ParamKind::Optional,
        }],
        block: None,
        ret: Box::new(Ty::Nil),
        effects: EffectSet::default(),
    };
    MethodDef {
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::with_default(attrs.clone(), attrs_default)],
        body: seq(stmts),
        signature: Some(signature),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

fn synth_attributes(owner: &ClassId, table: &Table) -> MethodDef {
    let entries: Vec<(Expr, Expr)> = table
        .columns
        .iter()
        .filter(|c| c.name.as_str() != "id")
        .map(|c| {
            let col_ty = ty_of_column(&c.col_type);
            (
                lit_sym(c.name.clone()),
                with_ty(
                    Expr::new(Span::synthetic(), ExprNode::Ivar { name: c.name.clone() }),
                    col_ty,
                ),
            )
        })
        .collect();

    // Hash<Sym, ?> — value type is a union of column types; collapsing to
    // Untyped is the conservative approximation. Refining to a Record
    // (row-polymorphic) is a follow-up if downstream wants per-key types.
    let hash_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, braced: true },
        ),
        hash_ty.clone(),
    );

    MethodDef {
        name: Symbol::from("attributes"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

fn synth_index_read(owner: &ClassId, table: &Table) -> MethodDef {
    let name = Symbol::from("name");

    let arms: Vec<crate::expr::Arm> = table
        .columns
        .iter()
        .map(|c| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Sym { value: c.name.clone() },
            },
            guard: None,
            body: Expr::new(Span::synthetic(), ExprNode::Ivar { name: c.name.clone() }),
        })
        .collect();

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Case {
            scrutinee: var_ref(name.clone()),
            arms,
        },
    );

    MethodDef {
        name: Symbol::from("[]"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(name.clone())],
        body,
        // Heterogeneous return (per-column type union); approximate as Untyped.
        signature: Some(fn_sig(vec![(name, Ty::Sym)], Ty::Untyped)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

fn synth_index_write(owner: &ClassId, table: &Table) -> MethodDef {
    let name = Symbol::from("name");
    let value = Symbol::from("value");

    let arms: Vec<crate::expr::Arm> = table
        .columns
        .iter()
        .map(|c| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Sym { value: c.name.clone() },
            },
            guard: None,
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: c.name.clone() },
                    value: var_ref(value.clone()),
                },
            ),
        })
        .collect();

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Case {
            scrutinee: var_ref(name.clone()),
            arms,
        },
    );

    MethodDef {
        name: Symbol::from("[]="),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(name.clone()), Param::positional(value.clone())],
        body,
        signature: Some(fn_sig(
            vec![(name, Ty::Sym), (value, Ty::Untyped)],
            Ty::Untyped,
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// Typed-Params update: takes the per-resource `<Resource>Params`
/// (typed slots for each permitted field) and assigns through the
/// model's `attr_writer` per field, **skipping fields whose value is
/// nil on the params object** (PATCH-style partial-update semantics).
///
/// The skip-nil pattern lets two construction shapes coexist:
///   - Controller path: `<Resource>Params.from_raw(@params)` populates
///     every field (defaults to `""` via `params.fetch(:k, "")`), so
///     `update` writes them all.
///   - Programmatic/test path: `<Resource>Params.new` followed by
///     selective setter calls leaves unset fields nil, and `update`
///     skips them — preserving Rails' partial-update idiom
///     (`record.update(title: "Renamed")` doesn't clobber body).
///
/// Save, return Bool.
fn synth_update_typed(owner: &ClassId, fields: &[Symbol]) -> MethodDef {
    let p = Symbol::from("p");
    let resource = Symbol::from(crate::naming::snake_case(owner.0.as_str()));
    let params_class_id = ClassId(Symbol::from(format!(
        "{}Params",
        crate::naming::camelize(resource.as_str())
    )));

    let mut stmts: Vec<Expr> = Vec::new();
    for field in fields {
        let p_field = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(p.clone())),
                method: field.clone(),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let nil_check = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(p_field.clone()),
                method: Symbol::from("nil?"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let assign_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![p_field],
                block: None,
                parenthesized: false,
            },
        );
        // `if p.<field>.nil? then nil else self.<field>= p.<field> end`
        // — equivalent to `self.<field> = p.<field> unless p.<field>.nil?`.
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: nil_check,
                then_branch: nil_lit(),
                else_branch: assign_call,
            },
        ));
    }

    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("save"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    ));

    let params_ty = Ty::Class { id: params_class_id, args: vec![] };
    MethodDef {
        name: Symbol::from("update"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(p.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(p, params_ty)], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

fn synth_update(owner: &ClassId, table: &Table) -> MethodDef {
    let attrs = Symbol::from("attrs");

    let mut stmts: Vec<Expr> = Vec::new();

    for col in &table.columns {
        if col.name.as_str() == "id" {
            continue;
        }

        let cond = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(attrs.clone())),
                method: Symbol::from("key?"),
                args: vec![lit_sym(col.name.clone())],
                block: None,
                parenthesized: true,
            },
        );

        let assign_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(var_ref(attrs.clone())),
                        method: Symbol::from("[]"),
                        args: vec![lit_sym(col.name.clone())],
                        block: None,
                        parenthesized: false,
                    },
                )],
                block: None,
                parenthesized: false,
            },
        );

        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond,
                then_branch: assign_call,
                else_branch: nil_lit(),
            },
        ));
    }

    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("save"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    ));

    let attrs_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    MethodDef {
        name: Symbol::from("update"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(attrs.clone())],
        body: seq(stmts),
        // save returns Bool.
        signature: Some(fn_sig(vec![(attrs, attrs_ty)], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}
