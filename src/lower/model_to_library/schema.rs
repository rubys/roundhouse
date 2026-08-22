//! Schema-driven methods: attr accessors, table_name, schema_columns,
//! instantiate, initialize, attributes, [], []=, update.

use crate::dialect::{AccessorKind, Association, MethodDef, MethodReceiver, Model, Param};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
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
    models: &[Model],
    table: &Table,
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
    // instead of each re-deriving a storage/accessor redirect. The
    // public `<col>=` writer normalizes through the `format_db_time`
    // intrinsic (the write-side sibling of `parse_db_time`/`db_now`,
    // native in every target runtime); hydration keeps writing stored
    // text via `<col>_raw=` directly.
    for col in &table.columns {
        methods.push(synth_attr_reader(owner, col));
        if is_temporal_col(col) {
            methods.push(synth_raw_reader(owner, col));
            // Rails-parity Time-accepting writer (lobsters' ban flow:
            // `self.banned_at = Time.now.utc`). A custom writer in the
            // model body must win, and `push_user_methods` runs after
            // this and drops collisions — so the synthesized writer
            // yields here (same dance as `synth_belongs_to_writer`).
            let writer_name = Symbol::from(format!("{}=", col.name.as_str()));
            if !super::associations::model_defines_instance_method(model, &writer_name) {
                methods.push(synth_temporal_writer(owner, col));
            }
        }
        methods.push(synth_attr_writer(owner, col));
        methods.push(synth_column_predicate(owner, col));
        // `<col>_previously_changed?` and `saved_change_to_<col>?`
        // (ActiveModel::Dirty subset) — both read the runtime Base's
        // `saved_changes` diff of the last save, and Rails documents
        // them as the same question, so they share one body builder
        // rather than drifting apart. Both spellings are synthesized
        // because the name is per-column: nothing static can answer
        // `saved_change_to_title?` from a single method, and
        // method_missing is off the table
        // ([[feedback_runtime_must_be_statically_resolvable]]).
        // `id` is answered by Base's own flag (it never appears in the
        // attributes hash the diff is built from).
        if col.name.as_str() != "id" {
            methods.push(synth_column_dirty_pred(owner, col, prev_changed_name(col)));
            methods.push(synth_column_dirty_pred(owner, col, saved_change_name(col)));
            methods.push(synth_column_prev_was(owner, col));
        }
    }

    // def self.table_name
    //
    // `model.table` — the name INGEST computed — not a second
    // derivation from the class name. The two disagree exactly where
    // Rails does: `pluralize_snake` keeps the namespace
    // (`push::subscriptions`) while Rails DEMODULIZES and prepends the
    // module parent's `table_name_prefix`, which is the entire reason
    // campfire's four-line `app/models/push.rb` exists. Ingest reads
    // both rules; this emitted the wrong name, so
    // `user.push_subscriptions.delete_all` issued `DELETE FROM
    // push::subscriptions` and SQLite answered "unrecognized token
    // ':'". Third copy of this rule found in one session
    // ([[feedback_port_dont_derive_inflections]]).
    methods.push(MethodDef {
        name: Symbol::from("table_name"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: lit_str(model.table.0.as_str().to_string()),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    });

    // def self.primary_key — emitted ONLY when the model overrode
    // Rails' default, so the runtime Base's `"id"` answers for everyone
    // else and no target pays a per-model method for the common case.
    if let Some(pk) = &model.primary_key {
        methods.push(MethodDef {
            name: Symbol::from("primary_key"),
            receiver: MethodReceiver::Class,
            params: Vec::new(),
            body: lit_str(pk.as_str().to_string()),
            signature: Some(fn_sig(vec![], Ty::Str)),
            effects: EffectSet::default(),
            enclosing_class: Some(owner.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }

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

    // def self.schema_time_columns — the temporal subset of the above.
    // JSON serialization is the consumer: Rails renders a temporal
    // attribute as ISO8601-with-offset while every other column renders
    // as its raw value, and the `[]` indexer hands back the STORED text
    // for both. Only the schema knows which is which, so the fact is
    // emitted rather than sniffed from the value at runtime.
    let time_column_array = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: table
                    .columns
                    .iter()
                    .filter(|c| is_temporal_col(c))
                    .map(|c| lit_sym(c.name.clone()))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        ),
        Ty::Array { elem: Box::new(Ty::Sym) },
    );
    methods.push(MethodDef {
        name: Symbol::from("schema_time_columns"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: time_column_array,
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
    methods.push(synth_from_row(owner, table, model_declares_after_initialize(model)));

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
    methods.push(synth_from_stmt(owner, table, model_declares_after_initialize(model)));

    // (No per-model `assign_from_row`: `Base#reload` dispatches to the
    // synthesized `_adapter_reload`, which re-reads the row off a
    // prepared statement and writes the column ivars directly. The
    // Hash-shaped `assign_from_row` contract survives only as Base's
    // no-op + the hand-written subclass in the framework base_test;
    // synthesizing it per model produced a dead method on every model
    // in every app — measured zero call sites in both the blog and
    // lobsters emits.)

    // def initialize(attrs = {}); super(); per-column self.col = attrs[:col] [|| 0 for id]; end
    methods.push(synth_initialize(owner, table, model, models));

    // def attributes; { col: @col, ... } excluding id; end
    methods.push(synth_attributes(owner, table));

    // def [](name); case name; when :col then @col; ...; end; end
    methods.push(synth_index_read(owner, table));

    // def []=(name, value); case name; when :col then @col = value; ...; end; end
    methods.push(synth_index_write(owner, table, model));

    // def update(attrs) / update!(attrs) — Rails-shaped: an attribute
    // Hash over the model's writable surface, saved.
    //
    // These used to be MONOMORPHIZED onto the canonical `<Resource>Params`
    // whenever any controller permitted this resource, which conflated two
    // different contracts. `<Resource>Params` is a MASS-ASSIGNMENT
    // BOUNDARY: deliberately narrower than the model's writers, and one
    // resource can carry several mutually-unrelated lists. `update` is not
    // that boundary — Rails guards at the params layer precisely so
    // `update` doesn't have to — and the model's own code calls it with
    // keys no permit list contains (campfire's `User#deactivate` writes
    // `status:`, which no controller permits, with a Symbol into an
    // integer enum). No `from_attrs` bridge can close that gap: the slots
    // don't exist, and the values are Strings, Symbols, Integers, nils,
    // Times and Hashes where a params slot is `Str`.
    //
    // So the typed assignment keeps its own name for every permit list
    // (`update_from_<class>` via `push_update_typed_variants`), the
    // controller call sites the lowerer already rewrites are retargeted
    // there (`rewrite_update_to_typed_variant`), and `update` stays
    // Rails-shaped for everyone else.
    for bang in [false, true] {
        methods.push(synth_update_hash(owner, model, table, bang));
    }

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
/// hand-written body already presents to the rust `str_color`
/// ownership pass, so no new clone-insertion handling is needed.
/// Names of the optional synthesized per-model surface — methods this
/// module creates for every model that exist only in case app code
/// calls them, and that tree-shake may therefore drop when
/// unreachable: the per-column presence predicates (`<col>?`), the
/// per-column dirty predicates (`<col>_previously_changed?`), and the
/// `update!` bang variant. Everything else synthesized here is load-
/// bearing framework contract (adapter primitives, row hydration,
/// lifecycle hooks — called from runtime `Base` bodies via bare
/// sends) and must never appear in this list.
///
/// Derives names with the same format strings the synthesizers use,
/// against the same column set, so the list can't drift from the
/// synthesis. Measured (blog + lobsters emits): these families are
/// where every synthesized-but-dead model method lives.
pub fn shakeable_synthesized_names(table: &Table) -> Vec<Symbol> {
    let mut names: Vec<Symbol> = Vec::new();
    for col in &table.columns {
        // Mirrors `synth_column_predicate` (pushed for every column).
        names.push(Symbol::from(format!("{}?", col.name.as_str())));
        // Mirrors the two `synth_column_dirty_pred` spellings (both
        // skipped for `id`, which Base answers from its own flag).
        // Shares the synthesizers' own name helpers, so a rename can't
        // silently strand one of them here.
        if col.name.as_str() != "id" {
            names.push(prev_changed_name(col));
            names.push(saved_change_name(col));
            names.push(prev_was_name(col));
        }
    }
    // Mirrors `synth_update_typed(.., bang: true)`.
    names.push(Symbol::from("update!"));
    names
}

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
    // The ivar's real type — nullable columns hold nil, and the
    // comparison below has to be against the value a strict target
    // actually stores (comparing an `Option<String>` to `""` doesn't
    // typecheck). `to_s` before the comparison keeps the Ruby reading
    // identical (`nil.to_s == ""`) and gives strict targets a scalar.
    let slot_ty = super::ty_of_column_slot(col);
    let nilable = matches!(slot_ty, Ty::Union { .. });
    // The nil-excluded value. `Cast` is the IR's narrowing bridge: the
    // ruby family unwraps it to the bare read, Crystal renders `.as(T)`,
    // rust unwraps the Option — all guarded by the `!nil?` conjunct
    // that precedes it, which short-circuits in every target.
    let scalar_read = |ty: Ty| {
        let read = col_ivar(col, if nilable { slot_ty.clone() } else { ty.clone() });
        if nilable {
            with_ty(
                Expr::new(Span::synthetic(), ExprNode::Cast { value: read, target_ty: ty.clone() }),
                ty,
            )
        } else {
            read
        }
    };
    let body = match &col_ty {
        // Boolean: the value's truthiness (`when true then true; false/nil
        // then false`).
        Ty::Bool => col_ivar(col, Ty::Bool),
        // Numeric: present AND non-zero (`!value.zero?`). `0` → false.
        // Numeric: present AND non-zero. A nullable numeric compares
        // its `to_s` against "0" for the same typecheck reason — the
        // nil case is already excluded by the first conjunct.
        Ty::Int | Ty::Float => and_bool(
            not_nil(col, &slot_ty),
            bool_send(scalar_read(col_ty.clone()), "!=", lit_int(0)),
        ),
        // String (and Date/DateTime/Time, which store as text →
        // `ty_of_column` Str): present AND non-empty (`!value.blank?`).
        // `""` → false — correct for a NULL datetime too, since
        // `column_text` hydrates SQL NULL as `""`, never `nil`. The `?`
        // predicate reads the stored text, not the `Time` reader.
        Ty::Str => and_bool(
            not_nil(col, &slot_ty),
            bool_send(scalar_read(Ty::Str), "!=", lit_str(String::new())),
        ),
        // Everything else (binary, json, references): present (`!nil?`).
        _ => not_nil(col, &slot_ty),
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
        // The slot type, not the bare column type: a nullable column
        // reads back nil until something sets it.
        let col_ty = super::ty_of_column_slot(col);
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

/// `def <col>=(value)` — the Rails-parity PUBLIC writer for a temporal
/// column: normalize the value to canonical storage text and store it
/// through the raw field.
///
///   def banned_at=(value)
///     self.banned_at_raw = ActiveSupport.format_db_time(value)
///   end
///
/// `format_db_time` (the write-side sibling of `parse_db_time` and
/// `db_now`, native in every target runtime) maps a `Time` → Rails'
/// exact storage form ("YYYY-MM-DD HH:MM:SS.ffffff", UTC), so every
/// write lands on the same on-disk format the reader parses. The
/// param's optionality follows COLUMN NULLABILITY: a nullable column
/// takes `Time | Nil` (nil clears it — the corpus shape,
/// `self.banned_at = Time.now.utc`), while a `null: false` column
/// takes a plain `Time` — its storage field is non-optional on strict
/// targets (Rust `String`, Kotlin `String`), so a nil-accepting param
/// would assign `Option<String>` into `String` (CI compare-rust/
/// kotlin/swift caught exactly that on blog's `null: false`
/// timestamps). Assigning nil to a NOT NULL column is unrepresentable
/// in the strict storage model — an honest subset of the Rails
/// in-memory-nil-until-save behavior. Stored-text writes go through
/// `<col>_raw=`. Void return and `AccessorKind::Method`, mirroring
/// `synth_belongs_to_writer` (a kind of `AttributeWriter` would read
/// as a plain field pair to per-target collapse walkers — and to the
/// ruby emit datetime pass's hand-written-writer arm — re-pointing
/// storage at a nonexistent `@<col>`). On the Ruby tree the
/// `self.<col>_raw =` attr-assign dispatches the raw writer, which is
/// where the emit-time parse-memo invalidation hooks in.
///
/// Shared home of what used to be the ruby-family emit pass's
/// synthesized-writer arm (`emit::ruby::library::apply_datetime_
/// lowering`); strict targets previously had NO public temporal writer
/// at all.
fn synth_temporal_writer(owner: &ClassId, col: &Column) -> MethodDef {
    let value_param = Symbol::from("value");
    let (value_ty, text_ty) = if col.nullable {
        (
            Ty::Union { variants: vec![Ty::Time, Ty::Nil] },
            Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
        )
    } else {
        (Ty::Time, Ty::Str)
    };
    let normalize = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                )),
                method: Symbol::from("format_db_time"),
                args: vec![with_ty(var_ref(value_param.clone()), value_ty.clone())],
                block: None,
                parenthesized: true,
            },
        ),
        text_ty.clone(),
    );
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Attr {
                    recv: Expr::new(Span::synthetic(), ExprNode::SelfRef),
                    name: col_storage_name(col),
                },
                value: normalize,
            },
        ),
        text_ty,
    );
    MethodDef {
        name: Symbol::from(format!("{}=", col.name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param.clone())],
        body,
        signature: Some(fn_sig(vec![(value_param, value_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

fn synth_attr_writer(owner: &ClassId, col: &Column) -> MethodDef {
    let value_param = Symbol::from("value");
    // Writers always take the STORAGE type and write the storage ivar:
    // `<col>=` / `@<col>` in general, `<col>_raw=` / `@<col>_raw` (Str)
    // for a temporal column. Every synthesized hydration path assigns
    // stored text, so this keeps the whole write side String-shaped.
    let col_ty = super::ty_of_column_slot(col);
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
    // historical `to_sym` step). Matches `synth_row_from_raw`. Internal
    // narrowing happens in the body.
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
    params_class_id: &ClassId,
    name: Symbol,
) {
    let owner = &model.name;
    let p = Symbol::from("p");
    let instance = Symbol::from("instance");

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
        let assign = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: field_storage_setter(table, field),
                args: vec![p_field.clone()],
                block: None,
                parenthesized: false,
            },
        );
        // The params class carries a `<field>_provided` flag. Rails'
        // `new(attrs)` never sees an absent key, so assigning one here
        // would write `""` over the column default.
        stmts.push({
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(var_ref(p.clone())),
                            method:
                                crate::lower::controller_to_library::params::provided_field(field),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        },
                    ),
                    then_branch: assign,
                    else_branch: nil_lit(),
                },
            )
        });
    }

    stmts.push(var_ref(instance));

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let params_ty = Ty::Class { id: params_class_id.clone(), args: vec![] };
    methods.push(MethodDef {
        name,
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

/// The `update` / `update!` pair for a NON-canonical permit list —
/// same bodies as the plain pair, named for the params class they take
/// (`update_from_users_profiles_user_params`). Two permit lists for one
/// resource are unrelated types on every strict target, so they can't
/// share a method name; the call-site rewrite
/// (`rewrite_update_to_typed_variant`) retargets the controller.
pub(super) fn push_update_typed_variants(
    methods: &mut Vec<MethodDef>,
    owner: &ClassId,
    fields: &[Symbol],
    table: &Table,
    spec: &crate::lower::controller_to_library::params::ParamsSpec,
) {
    use crate::lower::controller_to_library::params::model_update_name;
    for bang in [false, true] {
        methods.push(synth_update_typed(
            owner,
            fields,
            table,
            &spec.class_id,
            model_update_name(spec, bang),
        ));
    }
}

/// `def self.create_from_params(p); instance = <Model>.from_params(p);
/// instance.save; instance; end` — and the `!` variant with `save!`.
///
/// Rails' `create` is `new(attrs)` + save, and the runtime keeps that
/// shape over an attribute HASH. A call site handing it a typed params
/// object would reach `initialize(attrs)` and index a class with no
/// `[]`. This is the same call composed over the typed factory instead.
///
/// Why a method rather than rewriting the call site to
/// `<Model>.from_params(p).save!`: `save!` is declared on the runtime
/// Base and returns Base, so the chain's value types as the base class
/// and every downstream `@user.email_address` fails on a strict target.
/// The explicit `instance` tail keeps the concrete type — the same
/// reason `update!` ends in a `self` read rather than `save!`'s return.
pub(super) fn push_create_from_params_method(
    methods: &mut Vec<MethodDef>,
    owner: &ClassId,
    params_class_id: &ClassId,
    factory: Symbol,
    name: Symbol,
    bang: bool,
) {
    let p = Symbol::from("p");
    let instance = Symbol::from("instance");
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let from_params_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: factory,
            args: vec![var_ref(p.clone())],
            block: None,
            parenthesized: true,
        },
    );
    let stmts = vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: instance.clone() },
                value: from_params_call,
            },
        ),
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(if bang { "save!" } else { "save" }),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        ),
        var_ref(instance),
    ];

    let params_ty = Ty::Class { id: params_class_id.clone(), args: vec![] };
    methods.push(MethodDef {
        name,
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
fn synth_from_row(owner: &ClassId, table: &Table, fire_after_initialize: bool) -> MethodDef {
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
        // Slot type: for a nullable column both sides of this
        // assignment are nilable (the Row field and the model setter),
        // so casting to the bare type would unwrap on one side only.
        let col_ty = super::ty_of_column_slot(col);
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

    // The ActiveModel::Dirty baseline. A record hydrated from the DB
    // has just been given every column, and none of that is a CHANGE —
    // but `__track_saved_changes` diffs against a snapshot that was
    // still nil, so the first update reported `[nil, value]` for every
    // column and `<col>_previously_was` answered nil for all of them.
    // connection.rb's own comment called this out and deferred it
    // ("baseline-at-hydration is future work"); campfire's
    // `involvement_previously_was.inquiry.invisible?` is the caller
    // that needs it, and it needs the value rather than the predicate,
    // which is why nil showed rather than a wrong bool.
    //
    // A hook rather than an assignment here: the snapshot ivar belongs
    // to the ruby-family reopen, and `from_row` is synthesized for
    // every target. Base's no-op is what the strict lanes compile,
    // where the whole Dirty surface is already the empty subset.
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: Symbol::from("_note_hydrated"),
            args: vec![],
            block: None,
            // Parenthesized: go renders an unparenthesized zero-arg
            // Send as a method VALUE ("instance.NoteHydrated (value of
            // type func()) is not used"), not a call.
            parenthesized: true,
        },
    ));

    // Rails fires after_initialize on find/hydration too; same gate
    // as the synthesized initialize tail.
    if fire_after_initialize {
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from("after_initialize"),
                args: vec![],
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
fn synth_from_stmt(owner: &ClassId, table: &Table, fire_after_initialize: bool) -> MethodDef {
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
        let read_method = column_read_method_for(col);
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
    // Same Dirty baseline as `from_row` — `from_stmt` is the other
    // hydration factory, and a record read through it is just as much
    // "already saved" as one read through a row.
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: Symbol::from("_note_hydrated"),
            args: vec![],
            block: None,
            // Parenthesized: go renders an unparenthesized zero-arg
            // Send as a method VALUE ("instance.NoteHydrated (value of
            // type func()) is not used"), not a call.
            parenthesized: true,
        },
    ));

    // Rails fires after_initialize on find/hydration too; same gate
    // as the synthesized initialize tail.
    if fire_after_initialize {
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from("after_initialize"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ));
    }
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
        Ty::Float => "column_float",
        _ => "column_text",
    }
}

/// The `Db.column_*` primitive that hydrates this column. A nullable
/// column reads through the `_opt` variant so NULL arrives as nil
/// rather than the type's zero — `""` in a nullable UNIQUE column
/// collides row-to-row, and 0 in a nullable fk makes `where(fk: nil)`
/// match nothing. The primary key is never nullable in practice and is
/// excluded by `ty_of_column_slot`.
pub(super) fn column_read_method_for(col: &Column) -> &'static str {
    let base = ty_of_column(&col.col_type);
    if !matches!(super::ty_of_column_slot(col), Ty::Union { .. }) {
        return column_read_method(&base);
    }
    match base {
        Ty::Int => "column_int_opt",
        Ty::Bool => "column_bool_opt",
        Ty::Float => "column_float_opt",
        _ => "column_text_opt",
    }
}

/// `<col>_previously_changed?` — the ActiveModel::Dirty spelling that
/// reads as a property of the column (lobsters:
/// `merged_story_id_previously_changed?` in Story#log_moderations).
fn prev_changed_name(col: &Column) -> Symbol {
    Symbol::from(format!("{}_previously_changed?", col.name.as_str()))
}

/// `saved_change_to_<col>?` — the spelling that reads as a property of
/// the save (lobsters: `saved_change_to_selector?` in Domain). Rails
/// documents the two as the same question.
fn saved_change_name(col: &Column) -> Symbol {
    Symbol::from(format!("saved_change_to_{}?", col.name.as_str()))
}

/// `<col>_previously_was` — the VALUE half of the Dirty pair beside it.
/// The predicates answer WHETHER the last save changed the column;
/// this answers what it held BEFORE (campfire:
/// `@membership.involvement_previously_was.inquiry.invisible?`, which
/// decides whether a room appearing in a sidebar is a new grant or a
/// visibility change).
fn prev_was_name(col: &Column) -> Symbol {
    Symbol::from(format!("{}_previously_was", col.name.as_str()))
}

/// `def <col>_previously_was; saved_changes[:<col>] ... [0]; end` — the
/// previous value out of the last save's diff, whose entries are
/// `[prev, value]` pairs.
///
/// Answers nil when the column did not change, which is Rails: the
/// diff has no entry, and `attribute_previously_was` reads through it
/// rather than falling back to the current value.
///
/// Same `saved_changes` seam as the predicates, so the ruby-family
/// trees get the real snapshot diff from the connection.rb reopen and
/// the strict lanes get the honest empty-Hash subset — where this
/// answers nil for every column, as every Dirty predicate there
/// already answers false.
fn synth_column_prev_was(owner: &ClassId, col: &Column) -> MethodDef {
    // `self.attribute_previously_was(:<col>)` — the reader DELEGATES
    // rather than indexing the diff itself. The diff's entries are
    // `[prev, value]` pairs in a heterogeneous Hash, and an index into
    // one renders as an index on `interface{}` in go ("cannot index
    // __prev"), `object?` in C#, and the equivalent in every other
    // strict lane. Base answers nil there and the ruby-family reopen
    // does the indexing, which is the same split `saved_changes`
    // already takes.
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(self_ref()),
            method: Symbol::from("attribute_previously_was"),
            // STRING key, because `saved_changes` is a diff over
            // `attributes`, and Rails keys that by column-name String.
            args: vec![super::lit_str(col.name.as_str().to_string())],
            block: None,
            parenthesized: true,
        },
    );
    MethodDef {
        name: prev_was_name(col),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Untyped)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `def <name>; !saved_changes[:<col>].nil?; end` — the per-attribute
/// ActiveModel::Dirty predicate, answered from the runtime Base's
/// last-save diff. `name` selects which of the two equivalent spellings
/// above this instance carries; the body is identical either way.
fn synth_column_dirty_pred(owner: &ClassId, col: &Column, name: Symbol) -> MethodDef {
    // `!saved_changes[:<col>].nil?` rather than `.key?` — the diff's
    // values are always [prev, value] pairs so nil-of-missing-key IS
    // key absence, and the indexed read + nil-check renders natively
    // on every target (go's comma-ok `key?` idiom needs a Hash-typed
    // receiver, which models' class-info can't see through the
    // runtime-Base inheritance). Explicit self receiver: strict
    // emitters resolve receiverless Sends against the model's own
    // surface only.
    let saved_changes = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(self_ref()),
            method: Symbol::from("saved_changes"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let entry = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(saved_changes),
            method: Symbol::from("[]"),
            // STRING key — `saved_changes` diffs `attributes`, whose
            // keys are column-name Strings (Rails' own shape). A Symbol
            // here read a key that is never present, so every Dirty
            // predicate answered false: campfire's `Rooms::Open`
            // callback is guarded on `type_previously_changed?` and
            // stopped granting membership to anyone.
            args: vec![super::lit_str(col.name.as_str().to_string())],
            block: None,
            parenthesized: false,
        },
    );
    let is_nil = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(entry),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(is_nil),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    MethodDef {
        name,
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

/// True when the model's body declares an `after_initialize` hook —
/// as a block-form callback (Unknown item, possibly concern-spliced;
/// lobsters' Token concern) or its own `def after_initialize`. Gates
/// the hook-call tails in `synth_initialize` and the hydration
/// factories, so hook-free models (the whole blog fixture) emit
/// nothing new on any target.
pub(super) fn model_declares_after_initialize(model: &Model) -> bool {
    use crate::dialect::ModelBodyItem;
    model.body.iter().any(|item| match item {
        ModelBodyItem::Method { method, .. } => {
            method.name.as_str() == "after_initialize"
                && method.receiver == MethodReceiver::Instance
        }
        ModelBodyItem::Unknown { expr, .. } => matches!(
            &*expr.node,
            ExprNode::Send { recv: None, method, block: Some(_), .. }
                if method.as_str() == "after_initialize"
        ),
        _ => false,
    })
}

/// `self.<writer>(<lookup>) unless <lookup>.nil?` — the guarded
/// public-writer route `synth_initialize` uses for values that need
/// normalization (temporal columns) or foreign-key extraction
/// (belongs_to association objects). The nil guard keeps the writer's
/// typed parameter honest on strict targets: a missing attrs key never
/// reaches it.
fn assign_via_writer_unless_nil(writer: Symbol, lookup: Expr) -> Expr {
    let assign = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(self_ref()),
            method: writer,
            args: vec![lookup.clone()],
            block: None,
            parenthesized: false,
        },
    );
    guard_unless_nil(lookup, assign)
}

/// `<action> unless <lookup>.nil?` as IR (If with negated nil check,
/// Nil else-branch).
fn guard_unless_nil(lookup: Expr, action: Expr) -> Expr {
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(lookup),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let not_nil = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(nil_check),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: not_nil,
            then_branch: action,
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

fn synth_initialize(owner: &ClassId, table: &Table, model: &Model, models: &[Model]) -> MethodDef {
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
        // …except where the schema says the column is NULLABLE. Rails'
        // unset value there is NULL, and the type's zero is a different
        // value with different behaviour: `""` in a nullable UNIQUE
        // column collides on the second row (lobsters'
        // `users.password_reset_token`), and `0` in a nullable fk makes
        // `where(merged_story_id: nil)` — `scope :unmerged` — match
        // nothing. Those columns keep the bare lookup; the slot they
        // assign into is `Ty::Union{[T, Nil]}` (`ty_of_column_slot`),
        // so strict targets have somewhere to put the nil.
        // …and where the SCHEMA names a default, that is the value
        // Rails gives an unset attribute — `Membership.new.involvement`
        // is `"mentions"`, not nil, because `t.string "involvement",
        // default: "mentions"` says so. A nullable column with a
        // default therefore also takes the `||` form: its unset value
        // is the default, and only the type-zero case (no default
        // declared) leaves it NULL.
        let col_ty = ty_of_column(&col.col_type);
        let schema_default = schema_default_literal(col);
        let nullable = matches!(super::ty_of_column_slot(col), Ty::Union { .. })
            && schema_default.is_none();
        let default = schema_default.unwrap_or_else(|| default_literal_for_ty(&col_ty));
        // is_id_column reference retained as a feature flag for
        // future per-column override hooks; today every column flows
        // through the same default-lookup shape.
        let _ = is_id_column(&col.name);

        if is_temporal_col(col) {
            // Temporal columns: callers pass native `Time` values
            // (`Username.create!(created_at: user.created_at)` in
            // lobsters), so the attrs value can't be stuffed into the
            // raw ISO-text slot directly. First the STANDARD
            // `attrs[:col] || <default>` raw-slot assignment — the
            // exact pre-existing shape every target compiles (a Time
            // value transiently lands in the raw slot; nothing reads
            // between the two statements) — then a nil-guarded
            // normalize through the `format_db_time` intrinsic, the
            // same funnel `synth_temporal_writer` uses, called
            // directly because several emitters render the public
            // `<col>=` MethodDef without a property-setter
            // counterpart for the computed getter (tsc TS2540). On
            // the lenient ruby-family runtime the intrinsic passes
            // stored-text strings through unchanged. rust strips the
            // guard (let-binding constructor can't express it —
            // honest not-normalized subset); hydration is unaffected
            // (`from_row`/`from_stmt` write `<col>_raw=` directly).
            let standard = if nullable {
                lookup.clone()
            } else {
                Expr::new(
                    Span::synthetic(),
                    ExprNode::BoolOp {
                        op: crate::expr::BoolOpKind::Or,
                        surface: crate::expr::BoolOpSurface::Symbol,
                        left: lookup.clone(),
                        right: default,
                    },
                )
            };
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(self_ref()),
                    method: col_storage_setter(col),
                    args: vec![standard],
                    block: None,
                    parenthesized: false,
                },
            ));
            // Cast bridges the untyped attrs-hash value into the
            // intrinsic's Time parameter on strict targets (same seam
            // as `from_raw`'s adapter-row Cast); ruby-family emit
            // unwraps it to the inner value.
            let time_value = Expr::new(
                Span::synthetic(),
                ExprNode::Cast { value: lookup.clone(), target_ty: Ty::Time },
            );
            let normalized = with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            Span::synthetic(),
                            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                        )),
                        method: Symbol::from("format_db_time"),
                        args: vec![time_value],
                        block: None,
                        parenthesized: true,
                    },
                ),
                Ty::Str,
            );
            let raw_assign = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(self_ref()),
                    method: col_storage_setter(col),
                    args: vec![normalized],
                    block: None,
                    parenthesized: false,
                },
            );
            stmts.push(guard_unless_nil(lookup, raw_assign));
        } else {
            let value = if nullable {
                lookup
            } else {
                let defaulted = Expr::new(
                    Span::synthetic(),
                    ExprNode::BoolOp {
                        op: crate::expr::BoolOpKind::Or,
                        surface: crate::expr::BoolOpSurface::Symbol,
                        left: lookup,
                        right: default,
                    },
                );
                // `New(role: "administrator")` — the label reaches the
                // slot whole otherwise. Wrapped OUTSIDE the `||` so the
                // default still decides the absent case; the helper
                // passes an integer through untouched.
                enum_label_cast(model, col, defaulted.clone()).unwrap_or(defaulted)
            };
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
    }

    // Association-object keys: `Username.new(user: some_user)` — Rails
    // routes any attrs key through its public writer; the belongs_to
    // writer's whole body is "store the object's id in the fk", so
    // assign the fk column directly (`self.user_id = attrs[:user].id`)
    // — the writer-named Send form broke go, whose writer peephole
    // rewrites `self.user = …` to a field assignment no such field
    // backs. Cast bridges the untyped attrs value to the target class
    // for the `.id` read on strict targets. The fk column above
    // already defaulted (`attrs[:user_id] || 0`), so the object key
    // wins when provided; nil-guarded so a plain `new(user_id: 3)`
    // does nothing extra at runtime. Polymorphic belongs_to skipped —
    // the object route would also need the `<name>_type` column.
    for assoc in model.associations() {
        if let Association::BelongsTo { name, target, foreign_key, polymorphic: false, .. } = assoc
        {
            let lookup = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(attrs.clone())),
                    method: Symbol::from("[]"),
                    args: vec![lit_sym(name.clone())],
                    block: None,
                    parenthesized: false,
                },
            );
            let target_obj = Expr::new(
                Span::synthetic(),
                ExprNode::Cast {
                    value: lookup.clone(),
                    target_ty: Ty::Class { id: target.clone(), args: vec![] },
                },
            );
            let id_read = with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(target_obj),
                        method: Symbol::from("id"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
                Ty::Int,
            );
            let fk_assign = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(self_ref()),
                    method: Symbol::from(format!("{}=", foreign_key.as_str())),
                    args: vec![id_read],
                    block: None,
                    parenthesized: false,
                },
            );
            stmts.push(guard_unless_nil(lookup, fk_assign));
        }
    }

    // has_secure_password virtual attrs: `User.new(password: "...",
    // password_confirmation: "...")` is the factory/signup shape —
    // Rails routes them through the macro's plaintext writers (where
    // digest computation lives). The digest COLUMN was already covered
    // by the column loop above.
    if let Some(attr) = crate::lower::secure_password::secure_password_attr(&model.body) {
        for key in [attr.as_str().to_string(), format!("{}_confirmation", attr.as_str())] {
            let lookup = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(attrs.clone())),
                    method: Symbol::from("[]"),
                    args: vec![lit_sym(Symbol::from(key.clone()))],
                    block: None,
                    parenthesized: false,
                },
            );
            stmts.push(assign_via_writer_unless_nil(Symbol::from(format!("{key}=")), lookup));
        }
    }

    // `has_rich_text` virtual attrs: `Message.create!(body: "<div>hi
    // </div>")` is the canonical Rails spelling and the one a
    // controller reaches through, but `body` is not a column on THIS
    // table — the markup lives in `action_text_rich_texts` — so the
    // column loop above never saw it and mass assignment dropped it
    // silently. campfire posts a message that way and the message
    // persisted with no content at all.
    //
    // Routed through the synthesized `<attr>=` writer (which builds the
    // rich-text record and stages it for the after-save), exactly as
    // the secure-password block above routes through its plaintext
    // writers. `Cast` to `Str` bridges the untyped attrs value for the
    // strict targets, whose writer signature takes a String.
    for (_span, attr) in crate::lower::rich_text::rich_text_attrs(model) {
        let lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(attrs.clone())),
                method: Symbol::from("[]"),
                args: vec![lit_sym(attr.clone())],
                block: None,
                parenthesized: false,
            },
        );
        let value = Expr::new(
            Span::synthetic(),
            ExprNode::Cast { value: lookup.clone(), target_ty: Ty::Str },
        );
        let assign = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: Symbol::from(format!("{}=", attr.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        );
        stmts.push(guard_unless_nil(lookup, assign));
    }

    // has_many eager-load cache fields (issue #27): initialize each
    // `@<assoc>_cache = [] of <Target>` + `@<assoc>_loaded = false` so
    // the cache-aware reader's `@cache` reads/returns are non-nilable in
    // strict targets (Crystal types an ivar nilable unless it's assigned
    // in every initialize path). Harmless on dynamic targets. Mirrors the
    // ivar names in `associations::cache_ivar` / `loaded_ivar`.
    for assoc in model.associations() {
        if let Association::HasMany { name, target, through, .. } = assoc {
            // `has_many :through` collection writers stage into the
            // cache and flag the join rows stale — init the flag on
            // every construction path (mirrors cache/loaded below).
            // Only when the writer itself will exist: a nested chain
            // gets no writer (see `through_writer_join`), so no flag.
            let writer_synthesized = through.as_ref().is_some_and(|thr_name| {
                matches!(
                    super::associations::through_writer_join(model, models, thr_name, target),
                    super::associations::ThroughWriterJoin::Resolved(..)
                )
            });
            if writer_synthesized {
                let false_lit = with_ty(
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Bool { value: false } },
                    ),
                    Ty::Bool,
                );
                stmts.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Ivar {
                            name: Symbol::from(format!("{}_stale", name.as_str())),
                        },
                        value: false_lit,
                    },
                ));
            }
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

    // `after_initialize` tail — Rails fires the hook after construction
    // (and after find; the hydration factories append their own call).
    // Runs LAST so the hook observes the assigned attrs (Token's
    // generator checks `attributes.include?(:token)`).
    if model_declares_after_initialize(model) {
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("after_initialize"),
                args: vec![],
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
    // Keys are the PUBLIC column names as STRINGS, which is what Rails'
    // `record.attributes` answers — unambiguously, in every version.
    // Symbols here made the Rails idiom silently empty:
    // `attributes.slice("endpoint", …)` (campfire's
    // `users/push_subscriptions_controller_test`) sliced a Symbol-keyed
    // hash with String keys and got `{}`, and `assert_equal` said only
    // that it failed.
    //
    // The `[]` / `[]=` indexers stay monomorphic on Symbol: those are a
    // different API, deliberately narrowed
    // ([[feedback_monomorphize_polymorphic_apis]]), and a String call
    // site coerces at the lowering.
    //
    // Values read the storage ivar (`@col_raw` for temporal columns), so
    // `attributes` carries the stored-text form.
    let entries: Vec<(Expr, Expr)> = table
        .columns
        .iter()
        .filter(|c| c.name.as_str() != "id")
        .map(|c| {
            let col_ty = super::ty_of_column_slot(c);
            (super::lit_str(c.name.as_str().to_string()), col_ivar(c, col_ty))
        })
        .collect();

    // Hash<Str, ?> — value type is a union of column types; collapsing to
    // Untyped is the conservative approximation. Refining to a Record
    // (row-polymorphic) is a follow-up if downstream wants per-key types.
    let hash_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) };
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
            // NULLABLE columns only: wrap in `Cast` carrying the slot
            // type so a target that represents them as a reference
            // (Go's `*string`) can unbox — `record[:col]` yields the
            // value or nil, never a pointer. Every other column keeps
            // the bare read it always had; wrapping those too cost
            // them the ivar-read `.clone()` rust adds, moving out of
            // `&self`.
            body: {
                let read = Expr::new(
                    Span::synthetic(),
                    ExprNode::Ivar { name: col_storage_name(c) },
                );
                match super::ty_of_column_slot(c) {
                    slot @ Ty::Union { .. } => Expr::new(
                        Span::synthetic(),
                        ExprNode::Cast { value: read, target_ty: slot },
                    ),
                    _ => read,
                }
            },
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

/// A Rails ENUM column assigned by LABEL: `user.role = "administrator"`
/// on `enum :role, %i[member administrator bot]` has to store `1`.
///
/// `lower::enum_symbols` already translates a literal `role:
/// :administrator` at the call site, which covers everything an app
/// writes down. It cannot cover a value that only exists at runtime —
/// campfire's `Accounts::UsersController` reads the label off the
/// REQUEST (`params.require(:user)[:role].presence_in(%w[ member
/// administrator ])`), and the per-column `Cast` to `Int` turned it
/// into `"administrator".to_i` — **zero**, which is `member`. A role
/// change silently demoted the user instead of failing.
///
/// So the label→integer step moves to the one place that sees the
/// runtime value, and it stays a shared-runtime call rather than an
/// inlined table so all thirteen targets get one body.
///
/// `None` — leaving today's `Cast` — for a column that is not an enum,
/// for a NULLABLE one (the surrounding `|| <default>` / nil-guard
/// shapes decide nil there, and a helper that answers `0` for nil would
/// overwrite that decision), and for a mapping whose stored values are
/// not all integers (nothing in the corpus writes one, and a guess
/// would write the wrong cell).
fn enum_label_cast(model: &Model, col: &Column, raw: Expr) -> Option<Expr> {
    if matches!(super::ty_of_column_slot(col), Ty::Union { .. }) {
        return None;
    }
    let mapping = model.enums.get(&col.name)?;
    if mapping.is_empty() {
        return None;
    }
    let mut labels = Vec::new();
    let mut values = Vec::new();
    for (label, stored) in mapping {
        let Literal::Int { value } = stored else { return None };
        labels.push(with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Str { value: label.clone() } },
            ),
            Ty::Str,
        ));
        values.push(with_ty(
            Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: *value } }),
            Ty::Int,
        ));
    }
    let array = |elems: Vec<Expr>, of: Ty| {
        with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Array { elements: elems, style: Default::default() },
            ),
            Ty::Array { elem: Box::new(of) },
        )
    };
    Some(with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("ActiveRecord")] },
                )),
                method: Symbol::from("enum_int"),
                args: vec![
                    // `Cast` to Str is the bridge the strict targets
                    // use for every other attrs read; the ruby family
                    // unwraps it to the bare value, which compares
                    // equal to a label just the same.
                    with_ty(
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::Cast { value: raw, target_ty: Ty::Str },
                        ),
                        Ty::Str,
                    ),
                    array(labels, Ty::Str),
                    array(values, Ty::Int),
                ],
                block: None,
                parenthesized: true,
            },
        ),
        Ty::Int,
    ))
}

fn synth_index_write(owner: &ClassId, table: &Table, model: &Model) -> MethodDef {
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
            let col_ty = super::ty_of_column_slot(c);
            // `self[:role] = "administrator"` — same label problem the
            // attrs-hash writers have, same answer.
            let casted_value = enum_label_cast(model, c, var_ref(value.clone()))
                .unwrap_or_else(|| {
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Cast {
                            value: var_ref(value.clone()),
                            target_ty: col_ty,
                        },
                    )
                });
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

/// The literal a column's SCHEMA default names, typed by the COLUMN
/// rather than by how the default was spelled — lobsters declares a
/// decimal column `default: "0.0"`, and the slot wants a float, not
/// that string.
///
/// LIMIT, and it is at ingest rather than here: `parse_column_opts`
/// reads `default:` with `string_value`, so only a STRING literal is
/// captured at all. `default: 0` / `default: true` / `default: -> {
/// … }` arrive as `None` and fall through to the type-zero below,
/// which is the same value for every `default: 0` in the corpus and a
/// DIVERGENCE for lobsters' `default: true` and `default: 1`.
/// Widening the ingest is its own change with its own measurement —
/// it moves those columns' emitted constructors in a second app.
fn schema_default_literal(col: &Column) -> Option<Expr> {
    use crate::schema::ColumnType;
    let raw = col.default.as_ref()?;
    match &col.col_type {
        ColumnType::String { .. } | ColumnType::Text => Some(lit_str(raw.clone())),
        ColumnType::Integer | ColumnType::BigInt => raw.parse::<i64>().ok().map(lit_int),
        ColumnType::Float | ColumnType::Decimal { .. } => {
            raw.parse::<f64>().ok().map(|value| {
                with_ty(
                    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } }),
                    Ty::Float,
                )
            })
        }
        ColumnType::Boolean => match raw.as_str() {
            "true" | "t" | "1" => Some(with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Bool { value: true } },
                ),
                Ty::Bool,
            )),
            "false" | "f" | "0" => Some(with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Bool { value: false } },
                ),
                Ty::Bool,
            )),
            _ => None,
        },
        // Temporal / binary / json / reference defaults are SQL
        // expressions as often as they are literals (`-> { "DATETIME(
        // 'now') }`), and none of the corpus declares one that isn't.
        _ => None,
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
fn synth_update_typed(
    owner: &ClassId,
    fields: &[Symbol],
    table: &Table,
    params_class_id: &ClassId,
    name: Symbol,
) -> MethodDef {
    let p = Symbol::from("p");
    let bang = name.as_str().ends_with('!');

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
        // "Did the request provide this?" is the params class's own
        // flag — a value slot can't answer it, because a blank `""` and
        // an absent key are different facts Rails keeps apart.
        let nil_check = {
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(var_ref(p.clone())),
                            method:
                                crate::lower::controller_to_library::params::provided_field(field),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        },
                    )),
                    method: Symbol::from("!"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            )
        };
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
            method: Symbol::from(if bang { "save!" } else { "save" }),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    ));
    if bang {
        // update! returns the record — an explicit self read, not
        // save!'s Base-typed return (strict targets type the
        // difference).
        stmts.push(Expr::new(Span::synthetic(), ExprNode::SelfRef));
    }

    let params_ty = Ty::Class { id: params_class_id.clone(), args: vec![] };
    let ret_ty = if bang { Ty::Class { id: owner.clone(), args: vec![] } } else { Ty::Bool };
    MethodDef {
        name,
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(p.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(p, params_ty)], ret_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// Rails-shaped `update(attrs)` / `update!(attrs)` — an attribute Hash
/// over the model's WRITABLE surface, saved.
///
/// The field list is [`writable_field_set`], not `table.columns`: Rails'
/// `update` assigns through public writers, and a model's writers are
/// wider than its schema. campfire's `users(:david).update(password:
/// "…")` writes a `has_secure_password` virtual with no column behind
/// it; `Message.update(body: …)` writes a `has_rich_text` attr whose
/// markup lives in another table. Iterating columns alone did not fail
/// on those — it silently DROPPED them, so `assert users(:david).valid?`
/// passed having tested nothing. A hollow green is worse than the red,
/// which is why an unwritable key is now a diagnostic (see
/// `report_unwritable_update_keys`) rather than a quiet no-op.
///
/// Every statement shape here is lifted verbatim from
/// [`synth_initialize`] — the same `attrs[:k] || <default>` slot write,
/// the same two-step temporal normalize through `format_db_time`, the
/// same `Cast`-to-target `.id` write for a `belongs_to` object key. That
/// is deliberate: those shapes are the ones all eleven targets already
/// compile for mass assignment, so `update` introduces no new emit
/// surface. The one addition is the enclosing `attrs.key?(:k)` guard —
/// `update` is PATCH-shaped, and a key the caller omitted must not
/// overwrite the stored value with a type default.
///
/// `update!` differs only in the tail: `save!` (Base raises on invalid)
/// and an explicit `self` read, so the call's value keeps the concrete
/// model type instead of `save!`'s Base-typed return.
fn synth_update_hash(
    owner: &ClassId,
    model: &Model,
    table: &Table,
    bang: bool,
) -> MethodDef {
    let attrs = Symbol::from("attrs");

    let lookup = |field: &Symbol| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(attrs.clone())),
                method: Symbol::from("[]"),
                args: vec![lit_sym(field.clone())],
                block: None,
                parenthesized: false,
            },
        )
    };
    // `if attrs.key?(:field) then <body> end` — the PATCH guard.
    let when_present = |field: &Symbol, body: Expr| {
        let cond = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(attrs.clone())),
                method: Symbol::from("key?"),
                args: vec![lit_sym(field.clone())],
                block: None,
                parenthesized: true,
            },
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::If { cond, then_branch: body, else_branch: nil_lit() },
        )
    };

    let mut stmts: Vec<Expr> = Vec::new();

    for col in &table.columns {
        if col.name.as_str() == "id" {
            continue;
        }
        let slot_ty = super::ty_of_column_slot(col);
        // `Cast` to the column's SLOT type is the bridge from the
        // untyped attrs value on every strict target — the same node,
        // for the same reason, that `synth_index_write` wraps its
        // `value` param in (`self[:title] = v`). No `|| <default>`
        // fallback: this sits inside `attrs.key?`, so the value is
        // present by construction, and a nullable slot has somewhere to
        // put an explicit nil — which is what makes
        // `update!(unread_at: nil)` clear the column rather than stamp
        // a type zero.
        //
        // `synth_initialize`'s `attrs[:col] || <default>` shape is NOT
        // reusable here: rust compiles the constructor through a
        // let-binding path that coerces hash reads on its own, and
        // outside it a bare `serde_json::Value` reaches the typed setter
        // unconverted. The Hash-shaped `update` had never been compiled
        // by a strict target before — every corpus model carried a
        // permit list, so only the typed variant existed — which is how
        // that gap stayed invisible.
        // An enum column takes its LABEL here (`update(role:
        // "administrator")`) — the `Cast` to `Int` would read that as
        // zero. See `enum_label_cast`.
        let slot_value = enum_label_cast(model, col, lookup(&col.name)).unwrap_or_else(|| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Cast { value: lookup(&col.name), target_ty: slot_ty.clone() },
            )
        });
        let raw_assign = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: col_storage_setter(col),
                args: vec![slot_value],
                block: None,
                parenthesized: false,
            },
        );

        stmts.push(when_present(&col.name, raw_assign));
        if is_temporal_col(col) {
            // Normalize a native `Time` into the raw ISO-text slot
            // through `format_db_time`, the same funnel
            // `synth_initialize` uses. `update! last_active_at:
            // Time.now` is the campfire shape, and without this the slot
            // keeps a Time whose `to_s` is not Rails' storage form.
            //
            // A SIBLING statement, not nested inside the `key?` guard.
            // Its own nil check already subsumes the key check — an
            // absent key reads nil — and flat statements are what every
            // other synthesizer here emits. Nesting an `If` in
            // then-position is a shape the python emitter renders on one
            // line (`if a: if b: …`), which is a syntax error.
            let time_value = Expr::new(
                Span::synthetic(),
                ExprNode::Cast { value: lookup(&col.name), target_ty: Ty::Time },
            );
            let normalized = with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            Span::synthetic(),
                            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                        )),
                        method: Symbol::from("format_db_time"),
                        args: vec![time_value],
                        block: None,
                        parenthesized: true,
                    },
                ),
                Ty::Str,
            );
            let normalize_assign = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(self_ref()),
                    method: col_storage_setter(col),
                    args: vec![normalized],
                    block: None,
                    parenthesized: false,
                },
            );
            // The nil test rides the CAST value, not the raw hash read:
            // `nil?` on an untyped hash value is a known rust gap (it
            // renders `is_none()`, and `serde_json::Value` answers
            // `is_null()`), while the cast has already produced the
            // slot's own nilable type.
            let cast_for_guard = Expr::new(
                Span::synthetic(),
                ExprNode::Cast { value: lookup(&col.name), target_ty: slot_ty.clone() },
            );
            stmts.push(guard_unless_nil(cast_for_guard, normalize_assign));
        }
    }

    // Association-object keys (`membership.update!(room: other_room)`)
    // are DELIBERATELY not assignable here — the name is claimed so it
    // can't fall through to the virtual-writer loop, and nothing is
    // emitted for it.
    //
    // `synth_initialize` does support them, reading `attrs[:room].id`
    // through a `Cast` to the target class. That only compiles because
    // rust routes the constructor through a let-binding path that
    // strips the statement: an attrs Hash is `HashMap<String,
    // serde_json::Value>` on every strict target, and a model instance
    // cannot be a `serde_json::Value`. Emitting it from an ordinary
    // method makes the impossibility a compile error instead of a
    // silently-dropped statement.
    //
    // THE COST WAS NOT NIL, and this note used to say it was: the key
    // was SILENTLY DROPPED. campfire's `rooms(:designers).update!
    // creator: users(:jz)` wrote nothing, `can_administer?` stayed
    // false, and the test read as a permissions failure with nothing
    // pointing at mass assignment.
    //
    // `lower::assoc_attr_key` closes it at the CALL SITE — the one
    // place the record is still a typed expression rather than a hash
    // value — rewriting the key to the foreign-key column and the value
    // to its `id`, so what arrives here is an Integer and goes through
    // the column loop above. Giving the attrs Hash a value type that
    // can carry a record is still the deeper fix, and still a change to
    // the whole mass-assignment seam (initialize included) rather than
    // to `update` alone.
    let assoc_names: std::collections::BTreeSet<Symbol> = model
        .associations()
        .into_iter()
        .filter_map(|a| match a {
            Association::BelongsTo { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();

    // Everything else the model can be assigned through: `attr_accessor`
    // / `attr_writer` virtuals, `typed_store` attrs, the Attributes API,
    // `has_secure_password`'s plaintext pair, `has_rich_text` attrs.
    // Each has a synthesized `<attr>=` writer; route through it, exactly
    // as `synth_initialize` routes the password and rich-text keys.
    let rich_text: std::collections::BTreeSet<Symbol> =
        crate::lower::rich_text::rich_text_attrs(model).into_iter().map(|(_s, a)| a).collect();
    let mut virtuals = super::writable_field_set(model, table);
    // Hand-written `def <field>=` in the model body. `writable_field_set`
    // deliberately leaves these out — its callers hold a field name and
    // ask `model_defines_writer` per-field — but `update` has to
    // ENUMERATE, and a user-defined writer is exactly as assignable as a
    // synthesized one. lobsters' `def category_name=` is the shape, and
    // `permit_writer_filter` is the test that holds this pair together.
    for item in &model.body {
        let crate::dialect::ModelBodyItem::Method { method, .. } = item else { continue };
        if method.receiver != MethodReceiver::Instance {
            continue;
        }
        if let Some(base) = method.name.as_str().strip_suffix('=') {
            virtuals.insert(Symbol::from(base));
        }
    }
    for field in virtuals {
        if table.columns.iter().any(|c| c.name == field) || assoc_names.contains(&field) {
            continue;
        }
        // `Cast` to `Str` bridges the untyped attrs value into the
        // rich-text writer's String parameter on strict targets — the
        // one place `synth_initialize` needs it, and for the same
        // reason.
        let value = if rich_text.contains(&field) {
            Expr::new(
                Span::synthetic(),
                ExprNode::Cast { value: lookup(&field), target_ty: Ty::Str },
            )
        } else {
            lookup(&field)
        };
        let assign = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(self_ref()),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        );
        stmts.push(guard_unless_nil(lookup(&field), assign));
    }

    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(if bang { "save!" } else { "save" }),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    ));
    if bang {
        // Explicit `self` read — `save!` is declared on Base and returns
        // Base, so without this the call's value types as the base class
        // and every downstream typed read fails on a strict target.
        stmts.push(Expr::new(Span::synthetic(), ExprNode::SelfRef));
    }

    let attrs_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let ret_ty = if bang { Ty::Class { id: owner.clone(), args: vec![] } } else { Ty::Bool };
    MethodDef {
        name: Symbol::from(if bang { "update!" } else { "update" }),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(attrs.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(attrs, attrs_ty)], ret_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}
