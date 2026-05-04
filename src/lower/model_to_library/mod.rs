//! Lower a Rails-shape `Model` (with associations, validations, callbacks,
//! Schema-derived columns) into a post-lowering `LibraryClass` whose body
//! is a flat sequence of `MethodDef`s — the universal IR shape every
//! emitter consumes (see `project_universal_post_lowering_ir.md`).
//!
//! The output target is `fixtures/spinel-blog/app/models/<model>.rb`:
//! explicit method bodies (`def title; @title; end`, `def comments;
//! Comment.where(article_id: @id); end`, `def validate;
//! validates_presence_of(:title) { @title }; end`), no Rails DSL.
//!
//! This module is pure: input is one `Model` plus the app `Schema`, output
//! is one `LibraryClass`. No side-effects, no per-target choices. Per-Rails-
//! idiom lowering is a separate function so each can be tested in
//! isolation (skeleton, schema columns, has_many, belongs_to, validates,
//! callbacks, …).
//!
//! Strangler-fig direction: this lives alongside the existing per-target
//! emit paths. Callers that consume the post-lowering shape opt in
//! explicitly; the rich `Model` dialect remains the input for emitters
//! that haven't migrated.

mod schema;
mod validations;
mod associations;
mod broadcasts;
mod markers;
pub mod row;

use std::collections::HashMap;

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Model};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::{Row, Ty};

use self::associations::{push_association_methods, push_dependent_destroy};
use self::broadcasts::push_broadcasts_methods;
use self::markers::{push_block_callback_methods, push_unknown_marker_methods};
use self::schema::push_schema_methods;
use self::validations::push_validate_method;

/// Bulk entry point: lower every model in `models` against `schema`,
/// sharing one class registry so cross-model dispatch (`Article` calling
/// `Comment.where(...)`) types correctly. Use this for whole-app emit;
/// the single-model entry point below is for tests/probes.
///
/// `extra_class_infos` lets callers register additional ClassInfo
/// entries (e.g. lowered view modules so `Views::Articles.article(...)`
/// dispatches type) — passed as flat `(ClassId, ClassInfo)` pairs;
/// callers that want both the full path and a last-segment alias
/// should insert both.
/// Same as [`lower_models_to_library_classes`] but also returns the
/// shared class registry the body-typer used. Callers (e.g. the
/// controller lowerer) can extend that registry with their own
/// entries to keep cross-class dispatch resolving consistently.
pub fn lower_models_with_registry(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> (Vec<LibraryClass>, HashMap<ClassId, crate::analyze::ClassInfo>) {
    let (lcs, classes) = lower_models_inner(models, schema, extra_class_infos, &Default::default());
    (lcs, classes)
}

/// Variant that also takes (resource → permitted fields) tuples
/// collected from controllers. When a model's resource (e.g. `:article`
/// for `Article`) is in `params_specs`, the model gets a typed
/// `from_params(p: <Resource>Params)` factory whose body assigns each
/// permitted field through the column setter. Models without a
/// matching spec skip the factory (no controller permits them).
pub fn lower_models_with_registry_and_params(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
    params_specs: &std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>>,
) -> (Vec<LibraryClass>, HashMap<ClassId, crate::analyze::ClassInfo>) {
    lower_models_inner(models, schema, extra_class_infos, params_specs)
}

pub fn lower_models_to_library_classes(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<LibraryClass> {
    lower_models_inner(models, schema, extra_class_infos, &Default::default()).0
}

pub fn lower_models_to_library_classes_with_params(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
    params_specs: &std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>>,
) -> Vec<LibraryClass> {
    lower_models_inner(models, schema, extra_class_infos, params_specs).0
}

fn lower_models_inner(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
    params_specs: &std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>>,
) -> (Vec<LibraryClass>, HashMap<ClassId, crate::analyze::ClassInfo>) {
    // Synthesize per-model `<Model>Row` LibraryClasses up front. These
    // need to appear in the class registry before model body-typing so
    // calls to `<Model>.from_row(row)` and `<Model>Row.from_raw(hash)`
    // resolve correctly.
    let row_classes = self::row::synthesize_row_classes(models, schema);

    let mut all_methods: Vec<(Vec<MethodDef>, ClassId, Option<&Table>, &Model)> = Vec::new();
    for model in models {
        let methods = build_methods(model, schema, params_specs);
        let table = schema.tables.get(&model.table.0);
        all_methods.push((methods, model.name.clone(), table, model));
    }

    let mut classes: HashMap<ClassId, crate::analyze::ClassInfo> = HashMap::new();
    for (methods, name, table, model) in &all_methods {
        let info = build_class_info(model, methods, *table);
        classes.insert(name.clone(), info);
    }
    // Register synthesized Row classes so dispatch on `Article.from_row(r)`
    // / `ArticleRow.from_raw(h)` resolves through the body-typer.
    for row_lc in &row_classes {
        classes.insert(row_lc.name.clone(), self::row::row_class_info(row_lc));
    }
    // Register synthesized Params classes (info-only — the actual class
    // is emitted by the controller lowerer). Needed so the model's
    // `from_params` body's `p.<field>` Send dispatches to the
    // `<Resource>Params` attr_reader signature.
    for (resource, fields) in params_specs {
        let class_id = ClassId(Symbol::from(format!(
            "{}Params",
            crate::naming::camelize(resource.as_str())
        )));
        let mut info = crate::analyze::ClassInfo::default();
        for field in fields {
            info.instance_methods
                .insert(field.clone(), fn_sig(vec![], Ty::Str));
            info.instance_method_kinds
                .insert(field.clone(), crate::dialect::AccessorKind::AttributeReader);
            let setter_name = Symbol::from(format!("{}=", field.as_str()));
            info.instance_methods.insert(
                setter_name.clone(),
                fn_sig(vec![(Symbol::from("value"), Ty::Str)], Ty::Str),
            );
            info.instance_method_kinds
                .insert(setter_name, crate::dialect::AccessorKind::AttributeWriter);
        }
        info.class_methods.insert(
            Symbol::from("from_raw"),
            fn_sig(
                vec![(
                    Symbol::from("params"),
                    Ty::Hash {
                        key: Box::new(Ty::Sym),
                        value: Box::new(Ty::Untyped),
                    },
                )],
                Ty::Class { id: class_id.clone(), args: vec![] },
            ),
        );
        info.class_method_kinds
            .insert(Symbol::from("from_raw"), crate::dialect::AccessorKind::Method);
        classes.insert(class_id, info);
    }
    // Framework runtime stubs — referenced from broadcasts_to expansions
    // but not part of any model. Mirrors runtime/ruby/broadcasts.rb's
    // public surface (each takes a kwargs hash and returns Nil).
    classes.insert(ClassId(Symbol::from("Broadcasts")), broadcasts_class_info());
    // Caller-supplied entries (typically the lowered view modules,
    // registered under both their full ClassId and a last-segment
    // alias for the typer's last-segment Const lookup).
    for (id, info) in extra_class_infos {
        classes.insert(id, info);
    }

    let mut out = Vec::new();
    for (mut methods, _, table, model) in all_methods {
        for method in &mut methods {
            type_method_body(method, &classes, table);
        }
        out.push(LibraryClass {
            name: model.name.clone(),
            is_module: false,
            parent: model.parent.clone(),
            includes: Vec::new(),
            methods,
            origin: None,
        });
    }
    // Type-check Row class method bodies too so the strict typing residual
    // check doesn't blow up. The Row class shares its column shape with
    // the model's table (schema columns map 1:1 to attr_accessor pairs),
    // so we look up the corresponding table by stripping the `Row` suffix
    // off the class name. The typer's `seed ivar_bindings from columns`
    // path then resolves `@id` / `@title` / etc. inside attr_reader bodies.
    let mut row_classes = row_classes;
    for row_lc in &mut row_classes {
        let model_name = row_lc.name.0.as_str().trim_end_matches("Row");
        let table = models
            .iter()
            .find(|m| m.name.0.as_str() == model_name)
            .and_then(|m| schema.tables.get(&m.table.0));
        for method in &mut row_lc.methods {
            type_method_body(method, &classes, table);
        }
    }
    // Append synthesized Row classes after the model classes. Per-target
    // emit walks `out` linearly and emits one file per LibraryClass; the
    // Row classes get their own files (`app/models/article_row.rb`,
    // `article_row.ts`, etc.).
    out.extend(row_classes);
    (out, classes)
}

/// Build a `ClassInfo` for a lowered LibraryClass — used to feed
/// view modules / runtime-class lowerings into the model lowerer's
/// shared registry. Each `MethodDef.signature` becomes an entry in
/// `class_methods` (for `MethodReceiver::Class`) or `instance_methods`
/// (for `MethodReceiver::Instance`).
pub fn class_info_from_library_class(lc: &LibraryClass) -> crate::analyze::ClassInfo {
    use crate::dialect::AccessorKind;
    use crate::expr::{ExprNode, LValue};

    // Collect ivar names assigned in any instance method body. A
    // method whose name matches one of these ivars (e.g. Validations'
    // `def errors` paired with `@errors = []`) shadows the field
    // declaration in TS emit; the body analyzer's force-parens rule
    // shouldn't add `()` to such calls since the field is read as
    // a property. Reclassify those Method entries as AttributeReader
    // so the typer treats them like field accesses.
    let mut ivar_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    fn collect_ivars(e: &crate::expr::Expr, out: &mut std::collections::HashSet<String>) {
        match &*e.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                out.insert(name.as_str().to_string());
                collect_ivars(value, out);
            }
            ExprNode::Assign { target, value } => {
                if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                    collect_ivars(recv, out);
                }
                collect_ivars(value, out);
            }
            ExprNode::Send { recv, args, block, .. } => {
                if let Some(r) = recv {
                    collect_ivars(r, out);
                }
                for a in args {
                    collect_ivars(a, out);
                }
                if let Some(b) = block {
                    collect_ivars(b, out);
                }
            }
            ExprNode::Seq { exprs } => {
                for x in exprs {
                    collect_ivars(x, out);
                }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                collect_ivars(cond, out);
                collect_ivars(then_branch, out);
                collect_ivars(else_branch, out);
            }
            ExprNode::Lambda { body, .. } => collect_ivars(body, out),
            _ => {}
        }
    }
    for m in &lc.methods {
        if matches!(m.receiver, MethodReceiver::Instance) {
            collect_ivars(&m.body, &mut ivar_names);
        }
    }

    let mut info = crate::analyze::ClassInfo::default();
    for m in &lc.methods {
        if let Some(sig) = &m.signature {
            // If a Method-kind instance method's name matches an ivar
            // in the class body, treat as AttributeReader for the
            // typer. The TS emit already drops such methods in favor
            // of the ivar-derived field declaration; the kind change
            // keeps force-parens from firing on call sites.
            let kind = if matches!(m.receiver, MethodReceiver::Instance)
                && matches!(m.kind, AccessorKind::Method)
                && ivar_names.contains(m.name.as_str())
            {
                AccessorKind::AttributeReader
            } else {
                m.kind
            };
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), kind);
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), kind);
                }
            }
        }
    }
    info
}

/// Single-model entry point: lower one `Model` (Rails-shape, with DSL
/// items in `body`) into a post-lowering `LibraryClass`. Builds a
/// class registry containing only this model — for whole-app emit
/// where cross-model dispatch needs typing, prefer
/// `lower_models_to_library_classes`.
///
/// `schema` supplies the column list for the model's table — needed for
/// the per-column accessors / `attributes` / `[]` / `[]=` / `update` /
/// `initialize` lowerings. Models whose table isn't in the schema (rare;
/// abstract or virtual) get only the non-schema-driven methods.
pub fn lower_model_to_library_class(model: &Model, schema: &Schema) -> LibraryClass {
    let mut methods = build_methods(model, schema, &Default::default());
    let table = schema.tables.get(&model.table.0);
    let class_info = build_class_info(model, &methods, table);
    let mut classes: HashMap<ClassId, crate::analyze::ClassInfo> = HashMap::new();
    classes.insert(model.name.clone(), class_info);
    // Register Row classes so `<Model>.from_row(r)` / `<Model>Row.from_raw(h)`
    // calls inside the model body type correctly. The synthesized Row
    // class itself is not returned by this entry point (single-class
    // shape) — callers that need both should use the bulk entry point.
    let row_lcs = self::row::synthesize_row_classes(std::slice::from_ref(model), schema);
    for row_lc in &row_lcs {
        classes.insert(row_lc.name.clone(), self::row::row_class_info(row_lc));
    }
    for method in &mut methods {
        type_method_body(method, &classes, table);
    }
    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
        origin: None,
    }
}

/// Untyped-body method synthesis — shared by the single-model and
/// bulk entry points. Body-typing is the caller's responsibility (it
/// needs the cross-model registry).
fn build_methods(
    model: &Model,
    schema: &Schema,
    params_specs: &std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>>,
) -> Vec<MethodDef> {
    let mut methods: Vec<MethodDef> = Vec::new();

    if let Some(table) = schema.tables.get(&model.table.0) {
        let resource = crate::ident::Symbol::from(crate::naming::snake_case(model.name.0.as_str()));
        let permitted_fields = params_specs.get(&resource).map(|v| v.as_slice());
        push_schema_methods(&mut methods, model, table, permitted_fields);
        // `from_params(p: <Resource>Params)` — typed factory matching the
        // (resource, fields) tuple a controller's `permit(...)` declared.
        // Skipped silently when the model isn't permitted by any
        // controller.
        if let Some(fields) = permitted_fields {
            self::schema::push_from_params_method(&mut methods, model, fields);
        }
    }

    push_validate_method(&mut methods, model);
    push_association_methods(&mut methods, model);
    push_dependent_destroy(&mut methods, model);
    push_unknown_marker_methods(&mut methods, model);
    push_broadcasts_methods(&mut methods, model);
    push_block_callback_methods(&mut methods, model);

    methods
}

/// Construct the `ClassInfo` for a lowered model: schema-derived
/// attribute row, plus instance/class method tables built from the
/// synthesized `MethodDef.signature`s and an ApplicationRecord
/// baseline (save / destroy / persisted? / errors / find / all /
/// where / count / exists? / find_by / destroy_all).
fn build_class_info(
    model: &Model,
    methods: &[MethodDef],
    table: Option<&Table>,
) -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    info.table = Some(model.table.clone());

    // Attributes row from schema columns.
    if let Some(t) = table {
        let mut row = Row::closed();
        for col in &t.columns {
            row.fields.insert(col.name.clone(), ty_of_column(&col.col_type));
        }
        info.attributes = row;
    }

    // Synthesized method signatures + kinds.
    for m in methods {
        if let Some(sig) = &m.signature {
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), m.kind);
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), m.kind);
                }
            }
        }
    }

    // ApplicationRecord baseline (subset of runtime/ruby/active_record/base.rb's
    // public API that synthesized model bodies actually call). Only insert
    // when not already overridden by the lowerer.
    let class_id = &model.name;
    let owner_ty = Ty::Class { id: class_id.clone(), args: vec![] };
    let owner_or_nil = Ty::Union {
        variants: vec![owner_ty.clone(), Ty::Nil],
    };
    let array_owner = Ty::Array { elem: Box::new(owner_ty.clone()) };
    let any_hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };

    // ApplicationRecord declares `id` and `id=` (the schema synthesizers
    // skip the id column because it's inherited from the base class).
    insert_default(&mut info.instance_methods, "id", fn_sig(vec![], Ty::Int));
    insert_default(
        &mut info.instance_methods,
        "id=",
        fn_sig(vec![(Symbol::from("value"), Ty::Int)], Ty::Int),
    );
    insert_default(&mut info.instance_methods, "save", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "save!", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "destroy", fn_sig(vec![], owner_ty.clone()));
    insert_default(&mut info.instance_methods, "destroyed?", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "persisted?", fn_sig(vec![], Ty::Bool));
    insert_default(
        &mut info.instance_methods,
        "mark_persisted!",
        fn_sig(vec![], Ty::Nil),
    );
    // `errors` returns the same shape Validations.rb's ivar holds —
    // `Array[untyped]` (the framework runtime uses a plain Ruby
    // Array, mutated via `<<`/`push` and read via `empty?`/`count`/
    // `each`). The juntos hand-written ApplicationRecord wrapped
    // this in an ErrorCollection class, but the transpiled framework
    // keeps it primitive. Type-aware Array dispatch (`empty?` →
    // `length === 0`, `each` → `forEach`, etc.) handles all the
    // call sites uniformly.
    insert_default(
        &mut info.instance_methods,
        "errors",
        fn_sig(vec![], Ty::Array { elem: Box::new(Ty::Untyped) }),
    );
    insert_default(&mut info.instance_methods, "valid?", fn_sig(vec![], Ty::Bool));
    insert_default(
        &mut info.instance_methods,
        "reload",
        fn_sig(vec![], owner_ty.clone()),
    );

    // Validations mixin — instance helpers expected on every record.
    insert_default(
        &mut info.instance_methods,
        "validates_presence_of",
        fn_sig(vec![(Symbol::from("attr"), Ty::Sym), (Symbol::from("value"), Ty::Untyped)], Ty::Nil),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_absence_of",
        fn_sig(vec![(Symbol::from("attr"), Ty::Sym), (Symbol::from("value"), Ty::Untyped)], Ty::Nil),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_length_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_format_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_numericality_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_inclusion_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_belongs_to",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("fk_value"), Ty::Int),
                (Symbol::from("target_class"), Ty::Untyped),
            ],
            Ty::Nil,
        ),
    );

    // Class-level finders / scopes.
    insert_default(
        &mut info.class_methods,
        "find",
        fn_sig(vec![(Symbol::from("id"), Ty::Int)], owner_ty.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "find_by",
        fn_sig(vec![(Symbol::from("attrs"), any_hash.clone())], owner_or_nil),
    );
    insert_default(
        &mut info.class_methods,
        "all",
        fn_sig(vec![], array_owner.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "where",
        fn_sig(vec![(Symbol::from("conditions"), any_hash.clone())], array_owner.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "count",
        fn_sig(vec![], Ty::Int),
    );
    insert_default(
        &mut info.class_methods,
        "exists?",
        fn_sig(vec![(Symbol::from("id"), Ty::Int)], Ty::Bool),
    );
    insert_default(
        &mut info.class_methods,
        "destroy_all",
        fn_sig(vec![], Ty::Int),
    );
    insert_default(
        &mut info.class_methods,
        "first",
        fn_sig(vec![], Ty::Union { variants: vec![owner_ty.clone(), Ty::Nil] }),
    );
    insert_default(
        &mut info.class_methods,
        "last",
        fn_sig(vec![], Ty::Union { variants: vec![owner_ty.clone(), Ty::Nil] }),
    );
    insert_default(
        &mut info.class_methods,
        "new",
        fn_sig(vec![(Symbol::from("attrs"), any_hash.clone())], owner_ty.clone()),
    );
    // `<Model>.create(attrs)` → instance; `<Model>.create!(attrs)`
    // raises on validation failure but returns instance otherwise.
    // Both registered so test bodies (which use the bang form) and
    // seeds (which my has-many rewrite produces) type cleanly
    // through the registry.
    insert_default(
        &mut info.class_methods,
        "create",
        fn_sig(vec![(Symbol::from("attrs"), any_hash.clone())], owner_ty.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "create!",
        fn_sig(vec![(Symbol::from("attrs"), any_hash)], owner_ty.clone()),
    );

    // Typed factory taking the synthesized `<Model>Row` (one typed slot
    // per schema column). The body-typer needs this signature to resolve
    // `Article.from_row(row_value)` calls cross-class — `synth_from_row`
    // installs the body, but the registry entry has to exist before the
    // body of any caller is typed.
    let row_class_id = self::row::row_class_id(&model.name);
    insert_default(
        &mut info.class_methods,
        "from_row",
        fn_sig(
            vec![(Symbol::from("row"), Ty::Class { id: row_class_id, args: vec![] })],
            owner_ty,
        ),
    );

    // Tag every entry the baseline added as Method (defaults match —
    // ApplicationRecord's `save`, `find`, `where`, etc. are all real
    // method calls with parens). The earlier loop over synthesized
    // methods already populated `*_method_kinds` from the per-method
    // `kind` field; this fills in any baseline names that weren't
    // overridden.
    use crate::dialect::AccessorKind;
    for name in info.class_methods.keys().cloned().collect::<Vec<_>>() {
        info.class_method_kinds.entry(name).or_insert(AccessorKind::Method);
    }
    for name in info.instance_methods.keys().cloned().collect::<Vec<_>>() {
        info.instance_method_kinds.entry(name).or_insert(AccessorKind::Method);
    }
    // Baseline names that are field-like on the transpiled framework
    // Base/Validations (backed by `@<name>` ivars set in
    // `initialize`) — override the Method default so the body
    // analyzer's force-parens skips them. Callers reading
    // `record.errors` need the field, not a method call.
    for field_name in ["errors", "id"] {
        info.instance_method_kinds
            .insert(Symbol::from(field_name), AccessorKind::AttributeReader);
    }
    info
}

fn insert_default(map: &mut HashMap<Symbol, Ty>, name: &str, sig: Ty) {
    map.entry(Symbol::from(name)).or_insert(sig);
}

/// Stub `ClassInfo` for the `Broadcasts` framework module. Each
/// helper takes a kwargs hash and returns Nil (per the Ruby
/// runtime). Only carrying signatures the model lowerer's
/// broadcasts expansion + block-form callbacks actually emit.
fn broadcasts_class_info() -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    let kwargs = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let sig = fn_sig(vec![(Symbol::from("opts"), kwargs)], Ty::Nil);
    use crate::dialect::AccessorKind;
    for name in ["prepend", "replace", "remove", "append"] {
        info.class_methods.insert(Symbol::from(name), sig.clone());
        info.class_method_kinds.insert(Symbol::from(name), AccessorKind::Method);
    }
    info
}

fn type_method_body(
    method: &mut MethodDef,
    classes: &HashMap<ClassId, crate::analyze::ClassInfo>,
    table: Option<&Table>,
) {
    let typer = crate::analyze::BodyTyper::new(classes);
    let mut ctx = crate::analyze::Ctx::default();
    if let Some(Ty::Fn { params, .. }) = &method.signature {
        for (param, sig) in method.params.iter().zip(params.iter()) {
            ctx.local_bindings.insert(param.name.clone(), sig.ty.clone());
        }
    }
    if let Some(enclosing) = &method.enclosing_class {
        ctx.self_ty = Some(Ty::Class {
            id: ClassId(enclosing.clone()),
            args: vec![],
        });
    }
    // Seed ivar_bindings from schema columns so bare `@title` reads
    // resolve to the column type. Same source the synthesizers used
    // for the fields themselves.
    if matches!(method.receiver, MethodReceiver::Instance) {
        if let Some(t) = table {
            for col in &t.columns {
                ctx.ivar_bindings
                    .insert(col.name.clone(), ty_of_column(&col.col_type));
            }
        }
    }
    // Opt-in to `recv: Some(SelfRef)` rewriting on bare Sends —
    // matches the pattern view_to_library uses.
    ctx.annotate_self_dispatch = true;
    typer.analyze_expr(&mut method.body, &ctx);
}

// ---------------------------------------------------------------------------
// Small ExprNode constructors used throughout. Each takes a synthetic span
// since lowered methods don't correspond to a single source location.
// ---------------------------------------------------------------------------

pub(super) fn lit_str(s: String) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } }),
        Ty::Str,
    )
}

pub(super) fn lit_sym(name: Symbol) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name } }),
        Ty::Sym,
    )
}

pub(super) fn lit_int(value: i64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } }),
        Ty::Int,
    )
}

pub(super) fn lit_float(value: f64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } }),
        Ty::Float,
    )
}

pub(super) fn nil_lit() -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        Ty::Nil,
    )
}

/// Attach a known type to an Expr. Lowerers use this when the type is
/// statically known by construction — avoiding a separate analyzer
/// pass to rediscover what we already knew.
pub(super) fn with_ty(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}

/// Schema column type → roundhouse `Ty`. Mirrors `ingest::model::ty_of_column`
/// — duplicated here to avoid making that internal helper public for one
/// caller. Keep them in sync; the mapping is small and stable.
pub(super) fn ty_of_column(t: &ColumnType) -> Ty {
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => {
            Ty::Class { id: ClassId(Symbol::from("Time")), args: vec![] }
        }
        ColumnType::Binary => Ty::Str,
        ColumnType::Json => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
        ColumnType::Reference { .. } => Ty::Int,
    }
}

/// Build a `Ty::Fn` signature from positional (name, type) pairs and a return type.
/// Effects default to pure — callers refine if needed (lifecycle hooks etc.).
pub(super) fn fn_sig(params: Vec<(Symbol, Ty)>, ret: Ty) -> Ty {
    Ty::Fn {
        params: params
            .into_iter()
            .map(|(name, ty)| crate::ty::Param {
                name,
                ty,
                kind: crate::ty::ParamKind::Required,
            })
            .collect(),
        block: None,
        ret: Box::new(ret),
        effects: crate::effect::EffectSet::pure(),
    }
}

pub(super) fn var_ref(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name })
}

pub(super) fn class_const(id: &ClassId) -> Expr {
    let path: Vec<Symbol> = id.0.as_str().split("::").map(Symbol::from).collect();
    Expr::new(Span::synthetic(), ExprNode::Const { path })
}

pub(super) fn self_ref() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::SelfRef)
}

pub(super) fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

pub(super) fn is_id_column(name: &Symbol) -> bool {
    let s = name.as_str();
    s == "id" || s.ends_with("_id")
}
