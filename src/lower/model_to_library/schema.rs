//! Schema-driven methods: attr accessors, table_name, schema_columns,
//! instantiate, initialize, attributes, [], []=, update.

use crate::dialect::{AccessorKind, Association, MethodDef, MethodReceiver, Model, Param};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, Literal};
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
    //
    // Temporal (Date/DateTime/Time) columns split storage from access
    // AT THE IR LEVEL: the stored ISO-8601 text lives in a `<col>_raw`
    // String accessor pair (an ordinary field on every target), and the
    // public `<col>` reader is a computed getter parsing that text into
    // a native `Time`. Every synthesized internal reference (hydration,
    // predicate, attributes, `[]`/`[]=`, fill_timestamps, `_adapter_*`)
    // targets `<col>_raw` — so per-target emitters render what they see
    // instead of each re-deriving a storage/accessor redirect. There is
    // deliberately NO public `<col>=` writer: hydration writes stored
    // text via `<col>_raw=`, and a Rails-parity Time-accepting writer
    // (needing a `format_db_time` intrinsic) can be layered on when an
    // app needs one.
    for col in &table.columns {
        methods.push(synth_attr_reader(owner, col));
        if is_temporal_col(col) {
            methods.push(synth_raw_reader(owner, col));
        }
        methods.push(synth_attr_writer(owner, col));
        methods.push(synth_column_predicate(owner, col));
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
        is_async: false,
            mutates_self: false,
            block_param: None,
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
        is_async: false,
            mutates_self: false,
            block_param: None,
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

    // def self.from_stmt(stmt); instance = new; instance.<col> = Db.column_*(stmt, i); ...; mark_persisted!; instance; end
    //
    // Positional-path twin of `from_row`. Where `from_row` takes a
    // typed `<Model>Row` (the Hash/gem-adapter boundary), `from_stmt`
    // reads straight off a prepared-statement handle via the per-target
    // `Db.column_*` surface — no intermediate Row allocation on the hot
    // read path. Hydrates the full schema-column set in declaration
    // order at offset 0, so the SELECT feeding it MUST project every
    // column in that order (`ColumnSpec::All`). The Arel visitor only
    // routes `All`-projection hydrate sites here; a future `Named`
    // (partial/reordered) projection stays on its own inline path.
    methods.push(synth_from_stmt(owner, table));

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
    methods.push(synth_initialize(owner, table, model));

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
        Some(fields) => synth_update_typed(owner, fields, table),
        None => synth_update(owner, table),
    });

    // def fill_timestamps(creating); now = ActiveSupport.db_now; @updated_at = now; @created_at = now if creating; end
    //
    // Residualizes `ActiveRecord::Base#fill_timestamps`, which probes the
    // schema at RUNTIME (`schema_columns.include?(:updated_at)`) on every
    // save. Column presence is a compile-time-constant fact, so the
    // per-model override drops the `include?` guards and emits only the
    // live assignments. Models with neither timestamp column get no
    // override and fall through to Base's (already-inert) generic version.
    if let Some(m) = synth_fill_timestamps(owner, table) {
        methods.push(m);
    }
}

/// Per-model `fill_timestamps(creating)` — the compile-time
/// residualization of `ActiveRecord::Base#fill_timestamps`. The Base
/// version reads `self.class.schema_columns` and tests
/// `.include?(:updated_at)` / `.include?(:created_at)` on every save;
/// those facts are statically known per model, so the override drops
/// the probes and emits only the live assignments:
///
///   def fill_timestamps(creating)
///     now = ActiveSupport.db_now
///     @updated_at_raw = now
///     @created_at_raw = now if creating
///   end
///
/// (Timestamps are temporal columns, so the stamps land on the
/// `<col>_raw` storage ivar — see `col_storage_name`.)
///
/// `ActiveSupport.db_now` is the write-side temporal intrinsic
/// (sibling of the read-side `parse_db_time`): current UTC time in
/// Rails' exact storage form, `YYYY-MM-DD HH:MM:SS.ffffff` — space
/// separator, zero-padded 6-digit fractional seconds, no zone marker.
/// Matching Rails byte-for-byte keeps a column's TEXT values
/// homogeneous when a roundhouse-emitted app writes into a
/// Rails-created database, which is what keeps lexicographic
/// (SQL TEXT) comparison and ORDER BY correct — the previous
/// `Time.now.utc.iso8601` form ("…T…Z", whole seconds) sorted after
/// every same-day Rails-form value and dropped sub-second precision.
///
/// Returns `None` for a model with neither timestamp column — it keeps
/// Base's generic version, whose two `include?` checks both return false
/// (already a no-op), so an empty override would be pure noise.
/// `updated_at` is stamped on every save, `created_at` only on insert
/// (`if creating`) — matching the Base semantics exactly. The `now`
/// local is used at up to two sites; that's the same shape Base's
/// hand-written body already presents to the rust2 `str_color`
/// ownership pass, so no new clone-insertion handling is needed.
fn synth_fill_timestamps(owner: &ClassId, table: &Table) -> Option<MethodDef> {
    let find_col = |n: &str| table.columns.iter().find(|c| c.name.as_str() == n);
    let updated_col = find_col("updated_at");
    let created_col = find_col("created_at");
    if updated_col.is_none() && created_col.is_none() {
        return None;
    }

    let creating = Symbol::from("creating");
    let now = Symbol::from("now");

    // now = ActiveSupport.db_now
    let db_now = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                )),
                method: Symbol::from("db_now"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        ),
        Ty::Str,
    );
    let mut stmts = vec![with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: now.clone() },
                value: db_now,
            },
        ),
        Ty::Str,
    )];

    // `@<storage> = now` — an Assign returning the (String) timestamp
    // value. Timestamps are temporal columns, so this lands on the
    // `<col>_raw` storage ivar.
    let assign_now = |col: &Column| {
        with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: col_storage_name(col) },
                    value: with_ty(var_ref(now.clone()), Ty::Str),
                },
            ),
            Ty::Str,
        )
    };

    // @updated_at_raw = now  (every save)
    if let Some(col) = updated_col {
        stmts.push(assign_now(col));
    }

    // @created_at_raw = now if creating  (insert only)
    if let Some(col) = created_col {
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: with_ty(var_ref(creating.clone()), Ty::Bool),
                then_branch: assign_now(col),
                else_branch: nil_lit(),
            },
        ));
    }

    Some(MethodDef {
        name: Symbol::from("fill_timestamps"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(creating.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(creating, Ty::Bool)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    })
}

/// Rails generates a `<column>?` predicate for every attribute. A boolean
/// column's predicate is the value's truthiness (`is_deleted?` →
/// `@is_deleted`); every other column's is a presence check (`deleted_at?`
/// → `!@deleted_at.nil?`). The `!nil?` form is exact for nil-vs-present and
/// correct for both nullable and NOT NULL columns (a non-null `@col` is
/// never nil, so the predicate is constant-true); the string-specific
/// empty-is-also-blank nuance of Rails' `present?` isn't modeled (rare, and
/// keeps the body trivially typed `Bool`).
///
/// Emitting `<col>?` for every column relies on each target's renderer
/// disambiguating the `?` suffix from the same-named reader (`deleted_at`
/// vs `deleted_at?`) — Ruby/Crystal/Elixir keep `?`, TS prepends `is_`,
/// Python suffixes `_p`, and the strip targets (Kotlin/Swift/C#/Go/Rust)
/// affix `Pred`/`_pred`. Before that was uniform this synthesizer fired for
/// boolean columns only (and no fixture has one, so it was never exercised
/// cross-target).
fn synth_column_predicate(owner: &ClassId, col: &Column) -> MethodDef {
    let col_ty = ty_of_column(&col.col_type);
    let body = match &col_ty {
        // Boolean: the value's truthiness (`when true then true; false/nil
        // then false`).
        Ty::Bool => col_ivar(col, Ty::Bool),
        // Numeric: present AND non-zero (`!value.zero?`). `0` → false.
        Ty::Int | Ty::Float => and_bool(
            not_nil(col, &col_ty),
            bool_send(col_ivar(col, col_ty.clone()), "!=", lit_int(0)),
        ),
        // String (and Date/DateTime/Time, which store as text →
        // `ty_of_column` Str): present AND non-empty (`!value.blank?`).
        // `""` → false — correct for a NULL datetime too, since
        // `column_text` hydrates SQL NULL as `""`, never `nil`. The `?`
        // predicate reads the stored text, not the `Time` reader.
        Ty::Str => and_bool(
            not_nil(col, &col_ty),
            bool_send(col_ivar(col, Ty::Str), "!=", lit_str(String::new())),
        ),
        // Everything else (binary, json, references): present (`!nil?`).
        _ => not_nil(col, &col_ty),
    };
    MethodDef {
        name: Symbol::from(format!("{}?", col.name.as_str())),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// A typed storage-ivar read for a column (`@col`, or `@col_raw` for a
/// temporal column — see `col_storage_name`).
fn col_ivar(col: &Column, ty: Ty) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: col_storage_name(col) }),
        ty,
    )
}

/// The ivar/field a column's value is STORED in. A temporal column
/// stores its ISO-8601 text under `<col>_raw` (the public `<col>` reader
/// is a computed getter parsing that text — see `synth_attr_reader`);
/// every other column stores under its own name. All synthesized
/// internal references go through this so the storage/accessor split is
/// explicit in the IR rather than re-derived per target at emit time.
pub fn col_storage_name(col: &Column) -> Symbol {
    if is_temporal_col(col) {
        Symbol::from(format!("{}_raw", col.name.as_str()))
    } else {
        col.name.clone()
    }
}

/// The setter-method name synthesized internal writes dispatch to
/// (`<col>=`, or `<col>_raw=` for a temporal column).
fn col_storage_setter(col: &Column) -> Symbol {
    Symbol::from(format!("{}=", col_storage_name(col).as_str()))
}

/// Storage setter for a field known only by name (permit lists). Falls
/// back to `<field>=` when the name isn't a schema column (virtual
/// attribute — `attr_accessor` writers keep their own name).
fn field_storage_setter(table: &Table, field: &Symbol) -> Symbol {
    match table.columns.iter().find(|c| c.name == *field) {
        Some(col) => col_storage_setter(col),
        None => Symbol::from(format!("{}=", field.as_str())),
    }
}

/// `recv.<method>` with no arguments or block (e.g. `@col.nil?`, `cond.!`).
fn no_arg_send(recv: Expr, method: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    )
}

/// `!@col.nil?` — a typed presence test.
fn not_nil(col: &Column, ty: &Ty) -> Expr {
    let nil_q = with_ty(no_arg_send(col_ivar(col, ty.clone()), "nil?"), Ty::Bool);
    with_ty(no_arg_send(nil_q, "!"), Ty::Bool)
}

/// `recv <op> arg` — a binary-operator Send typed `Bool` (used for `!=`).
fn bool_send(recv: Expr, op: &str, arg: Expr) -> Expr {
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(op),
                args: vec![arg],
                block: None,
                parenthesized: false,
            },
        ),
        Ty::Bool,
    )
}

/// `left && right`, typed `Bool`.
fn and_bool(left: Expr, right: Expr) -> Expr {
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: BoolOpKind::And,
                surface: BoolOpSurface::Symbol,
                left,
                right,
            },
        ),
        Ty::Bool,
    )
}

fn synth_attr_reader(owner: &ClassId, col: &Column) -> MethodDef {
    // Temporal columns store ISO-8601 TEXT (`ty_of_column` → Str) but
    // read back as a real `Time`: the reader parses the stored text so
    // `record.created_at` is a native `Time` for callers / analyze /
    // Rails-canonical JSON. This is the shared, all-target home of what
    // used to be Ruby's emit-only `apply_datetime_lowering`. Each backend
    // renders `parse_db_time` (a stored-text→Time intrinsic) natively; a
    // target that hasn't wired one yet surfaces the honest not-supported
    // gap on this reader's `Ty::Time` return type.
    let (body, ret_ty) = if is_temporal_col(col) {
        // Nilable: a stored value can be absent (NULL / unset), so the
        // parse short-circuits to nil. `Time?` is the honest static type
        // and matches what a strict-null target infers from the nilable
        // storage ivar.
        (
            temporal_reader_body(col),
            Ty::Union { variants: vec![Ty::Time, Ty::Nil] },
        )
    } else {
        let col_ty = ty_of_column(&col.col_type);
        (
            with_ty(
                Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.name.clone() }),
                col_ty.clone(),
            ),
            col_ty,
        )
    };
    MethodDef {
        name: col.name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], ret_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// True for a Date/DateTime/Time column — a stored-text column whose
/// reader parses to a native `Time`.
fn is_temporal_col(col: &Column) -> bool {
    matches!(
        col.col_type,
        crate::schema::ColumnType::Date
            | crate::schema::ColumnType::DateTime
            | crate::schema::ColumnType::Time
    )
}

/// `ActiveSupport.parse_db_time(@col_raw)` — reader body for a temporal
/// column. `parse_db_time` is nil-safe (nil / empty stored value → nil)
/// and reads a zone-less stored value as UTC, so no explicit `&&` guard
/// is needed — this renders cleanly on strict-null targets, where a
/// guard would force a nil-raising `.not_nil!`. Typed `Time | Nil`.
/// Every target (Ruby included) renders this same shape; each maps
/// `parse_db_time` to its native parse.
fn temporal_reader_body(col: &Column) -> Expr {
    let ivar = with_ty(
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: col_storage_name(col) }),
        Ty::Str,
    );
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                )),
                method: Symbol::from("parse_db_time"),
                args: vec![ivar],
                block: None,
                parenthesized: true,
            },
        ),
        Ty::Union { variants: vec![Ty::Time, Ty::Nil] },
    )
}

/// `<col>_raw` — the plain String reader over a temporal column's
/// storage ivar. Together with its writer (`synth_attr_writer` names
/// temporal writers `<col>_raw=`) this is an ordinary String accessor
/// pair, so every target declares the backing field through its normal
/// collapse path — no per-emitter storage redirect. It is also the
/// uniform stored-text escape hatch (a target without a native `Time`
/// seam can read/serialize the raw text honestly).
fn synth_raw_reader(owner: &ClassId, col: &Column) -> MethodDef {
    let name = col_storage_name(col);
    let body = with_ty(
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() }),
        Ty::Str,
    );
    MethodDef {
        name,
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn synth_attr_writer(owner: &ClassId, col: &Column) -> MethodDef {
    let value_param = Symbol::from("value");
    // Writers always take the STORAGE type and write the storage ivar:
    // `<col>=` / `@<col>` in general, `<col>_raw=` / `@<col>_raw` (Str)
    // for a temporal column. Every synthesized hydration path assigns
    // stored text, so this keeps the whole write side String-shaped.
    let col_ty = ty_of_column(&col.col_type);
    let rhs = with_ty(var_ref(value_param.clone()), col_ty.clone());
    // Assign expression evaluates to the RHS in Ruby; same in TS.
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: col_storage_name(col) },
                value: rhs,
            },
        ),
        col_ty.clone(),
    );
    MethodDef {
        name: col_storage_setter(col),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param.clone())],
        body,
        signature: Some(fn_sig(vec![(value_param, col_ty.clone())], col_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeWriter,
        is_async: false,
            mutates_self: false,
            block_param: None,
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
    // Adapter rows are String-keyed across all targets (Crystal/TS can't
    // dynamically create Symbols at runtime; Spinel adapters skip the
    // historical `to_sym` step). Matches `synth_row_from_raw` and
    // `synth_assign_from_row`. Internal narrowing happens in the body.
    let row_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) };
    MethodDef {
        name: Symbol::from("instantiate"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(row.clone())],
        body,
        signature: Some(fn_sig(vec![(row, row_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
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
    table: &Table,
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
                method: field_storage_setter(table, field),
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
        is_async: false,
            mutates_self: false,
            block_param: None,
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
        // row.<col> — typed accessor on <Model>Row. ArticleRow's
        // attr_readers are nilable (`property id : Int64?`), but
        // ActiveRecord::Base subclasses' inherited `id` (and
        // timestamp columns set in initialize) are non-nilable.
        // Wrap the row accessor in Cast to bridge the wider Row
        // type into the narrower model property — Crystal renders
        // as `row.id.as(Int64)`; TS as `row.id as number`; Spinel
        // unwraps to bare `row.id`.
        let col_ty = super::ty_of_column(&col.col_type);
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
        let cast_field = Expr::new(
            Span::synthetic(),
            ExprNode::Cast {
                value: row_field,
                target_ty: col_ty,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: col_storage_setter(col),
                args: vec![cast_field],
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
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// `def self.from_stmt(stmt); instance = new; instance.col = Db.column_*(stmt, i); ...; mark_persisted!; instance; end`
///
/// Reads each schema column positionally from a prepared-statement
/// handle (`stmt : Int`, the FFI int-as-ptr the `Db` surface uses) via
/// the type-appropriate `Db.column_int`/`column_bool`/`column_text`.
/// No `Cast` wrapping (unlike `from_row`): `column_*` returns the exact
/// non-nilable scalar each setter expects, so the types line up
/// directly. Marks the instance persisted before returning it.
fn synth_from_stmt(owner: &ClassId, table: &Table) -> MethodDef {
    let stmt = Symbol::from("stmt");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from("Db"));

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

    for (i, col) in table.columns.iter().enumerate() {
        // Db.column_*(stmt, i) — read method picked from the column's
        // type, mirroring the Arel visitor's `read_method_for`.
        let read_method = column_read_method(&ty_of_column(&col.col_type));
        let read_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(class_const(&db)),
                method: Symbol::from(read_method),
                args: vec![var_ref(stmt.clone()), lit_int(i as i64)],
                block: None,
                parenthesized: true,
            },
        );
        // instance.<col>= = Db.column_*(stmt, i)  (storage setter — a
        // temporal column's stored text lands on `<col>_raw=`)
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: col_storage_setter(col),
                args: vec![read_call],
                block: None,
                parenthesized: false,
            },
        ));
    }

    // instance.mark_persisted!
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: Symbol::from("mark_persisted!"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    ));
    stmts.push(var_ref(instance));

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    MethodDef {
        name: Symbol::from("from_stmt"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(stmt.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(stmt, Ty::Int)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// Schema-column `Ty` → the `Db.column_*` reader that yields it.
/// Mirrors `lower::arel::visitor::read_method_for`.
fn column_read_method(col_ty: &Ty) -> &'static str {
    match col_ty {
        Ty::Int => "column_int",
        Ty::Bool => "column_bool",
        _ => "column_text",
    }
}

/// `def assign_from_row(row); self.<col> = row[:<col>]; ...; end`
/// — mutates `self`, used by `ActiveRecord::Base#reload` after the
/// adapter re-fetches the row as a `Hash[Symbol, untyped]`. The Hash
/// stays Hash-shaped (no typed Row narrowing) since reload only
/// touches the existing instance's slots.
fn synth_assign_from_row(owner: &ClassId, table: &Table) -> MethodDef {
    let row = Symbol::from("row");
    // String-keyed row to match the SqliteAdapter row shape
    // (mirrors `synth_row_from_raw`). Crystal/TS can't dynamically
    // create Symbol keys at runtime; Spinel adapters skip the
    // historical `to_sym` step, so all targets see String keys.
    let row_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) };

    let mut stmts: Vec<Expr> = Vec::new();
    for col in &table.columns {
        // row["<col>"] — Hash index lookup keyed on the column-name string.
        let key = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Str { value: col.name.as_str().to_string() } },
            ),
            Ty::Str,
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
        // self.<col>= = row["<col>"]  (storage setter; adapter rows carry
        // stored text under the COLUMN name, so the key stays `<col>`)
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                method: col_storage_setter(col),
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
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

fn synth_initialize(owner: &ClassId, table: &Table, model: &Model) -> MethodDef {
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
        // Every column gets a `|| <type-default>` fallback. Ruby's
        // `Hash#[]` returns nil for missing keys, and `self.<col> =
        // nil` is fine in dynamic-typed targets — but strict-typed
        // targets (Rust) can't assign nil to a non-nullable column. By
        // surfacing the default at the IR level, all targets see the
        // same shape: `attrs[:col] || ""` for strings, `|| 0` for
        // ints/refs, etc. Ruby semantics survive unchanged
        // (`attrs[:col]` evaluates to the user-supplied value when
        // present and to the default otherwise — equivalent to the
        // pre-default lowering for present keys); strict targets get
        // the literal they need. The original id-specific path
        // (`|| 0` for id / `article_id`) was the precursor; this
        // generalizes the pattern to the whole column list.
        let col_ty = ty_of_column(&col.col_type);
        let default = default_literal_for_ty(&col_ty);
        let value = Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: crate::expr::BoolOpKind::Or,
                surface: crate::expr::BoolOpSurface::Symbol,
                left: lookup,
                right: default,
            },
        );
        // is_id_column reference retained as a feature flag for
        // future per-column override hooks; today every column flows
        // through the same default-lookup shape.
        let _ = is_id_column(&col.name);

        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: col_storage_setter(col),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        ));
    }

    // has_many eager-load cache fields (issue #27): initialize each
    // `@<assoc>_cache = [] of <Target>` + `@<assoc>_loaded = false` so
    // the cache-aware reader's `@cache` reads/returns are non-nilable in
    // strict targets (Crystal types an ivar nilable unless it's assigned
    // in every initialize path). Harmless on dynamic targets. Mirrors the
    // ivar names in `associations::cache_ivar` / `loaded_ivar`.
    for assoc in model.associations() {
        if let Association::HasMany { name, target, .. } = assoc {
            let elem = Ty::Class { id: target.clone(), args: vec![] };
            let empty = with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Array { elements: vec![], style: ArrayStyle::Brackets },
                ),
                Ty::Array { elem: Box::new(elem) },
            );
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) },
                    value: empty,
                },
            ));
            let false_lit = {
                let mut e = Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Bool { value: false } },
                );
                e.ty = Some(Ty::Bool);
                e
            };
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
                    value: false_lit,
                },
            ));
        }
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
        ExprNode::Hash { entries: Vec::new(), kwargs: false },
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
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

fn synth_attributes(owner: &ClassId, table: &Table) -> MethodDef {
    // Keys are the PUBLIC column names; values read the storage ivar
    // (`@col_raw` for temporal columns) — `attributes` carries the
    // stored-text form, matching the adapter write funnel it feeds.
    let entries: Vec<(Expr, Expr)> = table
        .columns
        .iter()
        .filter(|c| c.name.as_str() != "id")
        .map(|c| {
            let col_ty = ty_of_column(&c.col_type);
            (lit_sym(c.name.clone()), col_ivar(c, col_ty))
        })
        .collect();

    // Hash<Sym, ?> — value type is a union of column types; collapsing to
    // Untyped is the conservative approximation. Refining to a Record
    // (row-polymorphic) is a follow-up if downstream wants per-key types.
    let hash_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: false },
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
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

fn synth_index_read(owner: &ClassId, table: &Table) -> MethodDef {
    let name = Symbol::from("name");

    // Patterns match the PUBLIC column symbol; bodies read the storage
    // ivar (`@col_raw` for temporal) — `record[:created_at]` yields the
    // stored text, same as `attributes`.
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
                ExprNode::Ivar { name: col_storage_name(c) },
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
        name: Symbol::from("[]"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(name.clone())],
        body,
        // Heterogeneous return (per-column type union); approximate as Untyped.
        signature: Some(fn_sig(vec![(name, Ty::Sym)], Ty::Untyped)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

fn synth_index_write(owner: &ClassId, table: &Table) -> MethodDef {
    let name = Symbol::from("name");
    let value = Symbol::from("value");

    // Each branch assigns the untyped `value` param to a typed @ivar.
    // Wrap the RHS in a Cast IR node carrying the column's declared
    // type so strict-typed targets (Crystal `.as(T)`, future Rust
    // `try_into`) bridge the dispatch. Ruby/Spinel emit unwraps Cast
    // as the inner value (no cast operator); TS no-ops or emits
    // `(value as T)` depending on width.
    let arms: Vec<crate::expr::Arm> = table
        .columns
        .iter()
        .map(|c| {
            let col_ty = ty_of_column(&c.col_type);
            let casted_value = Expr::new(
                Span::synthetic(),
                ExprNode::Cast {
                    value: var_ref(value.clone()),
                    target_ty: col_ty,
                },
            );
            crate::expr::Arm {
                pattern: crate::expr::Pattern::Lit {
                    value: Literal::Sym { value: c.name.clone() },
                },
                guard: None,
                body: Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Ivar { name: col_storage_name(c) },
                        value: casted_value,
                    },
                ),
            }
        })
        .collect();

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Case {
            scrutinee: var_ref(name.clone()),
            arms,
        },
    );

    // Value/return types are a union of every column's type. Crystal
    // needs the value param annotated with this union so the per-arm
    // `.as(ColTy)` cast is provably reachable from the static type —
    // without it, Crystal narrows `value` to whatever single type
    // call sites pass and refuses casts to other column types. Return
    // is the same union (the case expression yields the assigned
    // value's per-arm type). Other targets either ignore the
    // annotation (Spinel/Ruby) or render the union equivalently.
    let value_ty = column_union_ty(table);
    // The case expression has no `else` arm, so Crystal infers the
    // return as the value-union plus Nil (unmatched name → Nil). Add
    // Nil to the declared return so the annotation matches.
    let return_ty = match &value_ty {
        Ty::Union { variants } => {
            let mut vs = variants.clone();
            vs.push(Ty::Nil);
            Ty::Union { variants: vs }
        }
        single => Ty::Union {
            variants: vec![single.clone(), Ty::Nil],
        },
    };

    MethodDef {
        name: Symbol::from("[]="),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(name.clone()), Param::positional(value.clone())],
        body,
        signature: Some(fn_sig(
            vec![(name, Ty::Sym), (value, value_ty)],
            return_ty,
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// Synth a type-appropriate default literal — used by
/// `synth_initialize` to back `attrs[:col] || <default>`. The result
/// is the value the column ivar receives when the constructor is
/// called without that key (Ruby `Article.new`, no args). Matches the
/// Rails ApplicationRecord convention (empty string for Str-shaped
/// columns including Time/DateTime stored as ISO strings, 0 for
/// Int/Float, false for Bool); Union-typed columns fall back to the
/// first variant's default.
fn default_literal_for_ty(ty: &Ty) -> Expr {
    use crate::expr::Literal;
    match ty {
        Ty::Str | Ty::Sym => lit_str(String::new()),
        Ty::Int => lit_int(0),
        Ty::Float => with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Float { value: 0.0 } },
            ),
            Ty::Float,
        ),
        Ty::Bool => with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Bool { value: false } },
            ),
            Ty::Bool,
        ),
        Ty::Hash { .. } => with_ty(
            Expr::new(Span::synthetic(), ExprNode::Hash { entries: Vec::new(), kwargs: false }),
            ty.clone(),
        ),
        Ty::Array { .. } => with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: Vec::new(),
                    style: crate::expr::ArrayStyle::default(),
                },
            ),
            ty.clone(),
        ),
        // Union / other: fall back to nil; strict targets handle the
        // residual but no current column type lands here.
        _ => nil_lit(),
    }
}

fn column_union_ty(table: &Table) -> Ty {
    use std::collections::BTreeSet;
    let mut variants: Vec<Ty> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for col in &table.columns {
        let ty = ty_of_column(&col.col_type);
        let key = format!("{ty:?}");
        if seen.insert(key) {
            variants.push(ty);
        }
    }
    if variants.len() == 1 {
        variants.into_iter().next().unwrap()
    } else {
        Ty::Union { variants }
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
fn synth_update_typed(owner: &ClassId, fields: &[Symbol], table: &Table) -> MethodDef {
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
                method: field_storage_setter(table, field),
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
        is_async: false,
            mutates_self: false,
            block_param: None,
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
                method: col_storage_setter(col),
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
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}
