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

use crate::dialect::{
    Association, Dependent, LibraryClass, MethodDef, MethodReceiver, Model, ModelBodyItem, Param,
    ValidationRule,
};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::pluralize_snake;
use crate::schema::{Schema, Table};
use crate::span::Span;

/// Entry point: take a `Model` (Rails-shape, with DSL items in `body`) and
/// produce the post-lowering `LibraryClass` whose `methods` carry every
/// Rails idiom expanded into explicit method bodies.
///
/// `schema` supplies the column list for the model's table — needed for
/// the per-column accessors / `attributes` / `[]` / `[]=` / `update` /
/// `initialize` lowerings. Models whose table isn't in the schema (rare;
/// abstract or virtual) get only the non-schema-driven methods.
pub fn lower_model_to_library_class(model: &Model, schema: &Schema) -> LibraryClass {
    let mut methods: Vec<MethodDef> = Vec::new();

    if let Some(table) = schema.tables.get(&model.table.0) {
        push_schema_methods(&mut methods, model, table);
    }

    push_validate_method(&mut methods, model);
    push_association_methods(&mut methods, model);
    push_dependent_destroy(&mut methods, model);
    push_unknown_marker_methods(&mut methods, model);
    // broadcasts_to expansion runs BEFORE block-form callbacks so the
    // expansion's emitted statements appear first in the composed
    // method body — matches spinel-blog's source order, where the
    // broadcasts_to-derived call leads and the explicit block-form
    // cascade follows.
    push_broadcasts_methods(&mut methods, model);
    push_block_callback_methods(&mut methods, model);

    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

// ---------------------------------------------------------------------------
// Schema-driven methods: attr accessors, table_name, schema_columns,
// instantiate, initialize, attributes, [], []=, update.
// ---------------------------------------------------------------------------

fn push_schema_methods(methods: &mut Vec<MethodDef>, model: &Model, table: &Table) {
    let owner = &model.name;

    // Per-column getter+setter for every non-id column. The id column
    // gets its accessors from ActiveRecord::Base; concrete models only
    // declare the per-table additions. (Spinel-blog's article.rb uses
    // `attr_accessor :title, :body, :created_at, :updated_at`.)
    for col in &table.columns {
        if col.name.as_str() == "id" {
            continue;
        }
        methods.push(synth_attr_reader(owner, &col.name));
        methods.push(synth_attr_writer(owner, &col.name));
    }

    // def self.table_name
    methods.push(MethodDef {
        name: Symbol::from("table_name"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: lit_str(pluralize_snake(model.name.0.as_str())),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    });

    // def self.schema_columns
    let column_array = Expr::new(
        Span::synthetic(),
        ExprNode::Array {
            elements: table
                .columns
                .iter()
                .map(|c| lit_sym(c.name.clone()))
                .collect(),
            style: ArrayStyle::Brackets,
        },
    );
    methods.push(MethodDef {
        name: Symbol::from("schema_columns"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: column_array,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    });

    // def self.instantiate(row); instance = new(row); instance.mark_persisted!; instance; end
    methods.push(synth_instantiate(owner));

    // def initialize(attrs = {}); super(); per-column self.col = attrs[:col] [|| 0 for id]; end
    methods.push(synth_initialize(owner, table));

    // def attributes; { col: @col, ... } excluding id; end
    methods.push(synth_attributes(owner, table));

    // def [](name); case name; when :col then @col; ...; end; end
    methods.push(synth_index_read(owner, table));

    // def []=(name, value); case name; when :col then @col = value; ...; end; end
    methods.push(synth_index_write(owner, table));

    // def update(attrs); per-non-id-column conditional setter; save; end
    methods.push(synth_update(owner, table));
}

fn synth_attr_reader(owner: &ClassId, name: &Symbol) -> MethodDef {
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: name.clone() },
    );
    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

fn synth_attr_writer(owner: &ClassId, name: &Symbol) -> MethodDef {
    let value_param = Symbol::from("value");
    let rhs = var_ref(value_param.clone());
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: name.clone() },
            value: rhs,
        },
    );
    MethodDef {
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

fn synth_instantiate(owner: &ClassId) -> MethodDef {
    let row = Symbol::from("row");
    let instance = Symbol::from("instance");

    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: vec![var_ref(row.clone())],
            block: None,
            parenthesized: true,
        },
    );

    let body = seq(vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: instance.clone() },
                value: new_call,
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

    MethodDef {
        name: Symbol::from("instantiate"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(row)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
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
    // `new_action` relies on. Without the default, callers hit
    // `wrong number of arguments (given 0, expected 1)`.
    let attrs_default = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries: Vec::new(), braced: true },
    );
    MethodDef {
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::with_default(attrs, attrs_default)],
        body: seq(stmts),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

fn synth_attributes(owner: &ClassId, table: &Table) -> MethodDef {
    let entries: Vec<(Expr, Expr)> = table
        .columns
        .iter()
        .filter(|c| c.name.as_str() != "id")
        .map(|c| {
            (
                lit_sym(c.name.clone()),
                Expr::new(Span::synthetic(), ExprNode::Ivar { name: c.name.clone() }),
            )
        })
        .collect();

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries, braced: true },
    );

    MethodDef {
        name: Symbol::from("attributes"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
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
        params: vec![Param::positional(name)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
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
        params: vec![Param::positional(name), Param::positional(value)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
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

    MethodDef {
        name: Symbol::from("update"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(attrs)],
        body: seq(stmts),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

// ---------------------------------------------------------------------------
// Validations: lower `validates :attr, presence: true, length: { ... }` into
// a single `def validate` body that calls `validates_presence_of(:attr) { @attr }`,
// `validates_length_of(:attr, minimum: N) { @attr }` etc. — block-yielding
// shape per the handoff. One top-level `def validate` per model; multiple
// rules across multiple attrs share the same method.
// ---------------------------------------------------------------------------

fn push_validate_method(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for v in model.validations() {
        for rule in &v.rules {
            stmts.extend(validation_rule_to_calls(&v.attribute, rule));
        }
    }

    if stmts.is_empty() {
        return;
    }

    methods.push(MethodDef {
        name: Symbol::from("validate"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: seq(stmts),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
    });
}

/// Produce the list of helper-call expressions for one `ValidationRule` on
/// `attr`. Each helper is `<helper>(:attr [, kwargs]) { @attr }` — the
/// block-yielding form is load-bearing per the handoff (must NOT
/// substitute `instance_variable_get`).
fn validation_rule_to_calls(attr: &Symbol, rule: &ValidationRule) -> Vec<Expr> {
    let attr_block = ivar_block(attr);
    match rule {
        ValidationRule::Presence => vec![helper_call(
            "validates_presence_of",
            vec![lit_sym(attr.clone())],
            attr_block,
        )],
        ValidationRule::Absence => vec![helper_call(
            "validates_absence_of",
            vec![lit_sym(attr.clone())],
            attr_block,
        )],
        ValidationRule::Length { min, max } => {
            let mut entries: Vec<(Expr, Expr)> = Vec::new();
            if let Some(n) = min {
                entries.push((lit_sym(Symbol::from("minimum")), lit_int(*n as i64)));
            }
            if let Some(n) = max {
                entries.push((lit_sym(Symbol::from("maximum")), lit_int(*n as i64)));
            }
            let mut args = vec![lit_sym(attr.clone())];
            args.push(Expr::new(
                Span::synthetic(),
                ExprNode::Hash { entries, braced: false },
            ));
            vec![helper_call("validates_length_of", args, attr_block)]
        }
        ValidationRule::Format { pattern } => vec![helper_call(
            "validates_format_of",
            vec![
                lit_sym(attr.clone()),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash {
                        entries: vec![(
                            lit_sym(Symbol::from("with")),
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit {
                                    value: Literal::Regex {
                                        pattern: pattern.clone(),
                                        flags: String::new(),
                                    },
                                },
                            ),
                        )],
                        braced: false,
                    },
                ),
            ],
            attr_block,
        )],
        ValidationRule::Numericality { only_integer, gt, lt } => {
            let mut entries: Vec<(Expr, Expr)> = Vec::new();
            if *only_integer {
                entries.push((
                    lit_sym(Symbol::from("only_integer")),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Bool { value: true } },
                    ),
                ));
            }
            if let Some(n) = gt {
                entries.push((lit_sym(Symbol::from("greater_than")), lit_float(*n)));
            }
            if let Some(n) = lt {
                entries.push((lit_sym(Symbol::from("less_than")), lit_float(*n)));
            }
            let mut args = vec![lit_sym(attr.clone())];
            if !entries.is_empty() {
                args.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries, braced: false },
                ));
            }
            vec![helper_call("validates_numericality_of", args, attr_block)]
        }
        ValidationRule::Inclusion { values } => {
            let array = Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: values
                        .iter()
                        .map(|lit| {
                            Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() })
                        })
                        .collect(),
                    style: ArrayStyle::Brackets,
                },
            );
            let entries = vec![(lit_sym(Symbol::from("in")), array)];
            vec![helper_call(
                "validates_inclusion_of",
                vec![
                    lit_sym(attr.clone()),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Hash { entries, braced: false },
                    ),
                ],
                attr_block,
            )]
        }
        ValidationRule::Uniqueness { .. } | ValidationRule::Custom { .. } => {
            // Not yet exercised by real-blog; lands when a fixture forces the issue.
            Vec::new()
        }
    }
}

fn helper_call(name: &str, args: Vec<Expr>, block: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(name),
            args,
            block: Some(block),
            parenthesized: true,
        },
    )
}

/// Produce the `{ @attr }` block lambda used by every validates_* helper.
fn ivar_block(attr: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: Vec::new(),
            block_param: None,
            body: Expr::new(Span::synthetic(), ExprNode::Ivar { name: attr.clone() }),
            block_style: crate::expr::BlockStyle::Brace,
        },
    )
}

// ---------------------------------------------------------------------------
// Associations: has_many becomes a typed reader returning a where-style
// query. dependent: :destroy generates a `before_destroy` cascade that
// iterates and destroys each child.
// ---------------------------------------------------------------------------

fn push_association_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let owner = &model.name;
    for assoc in model.associations() {
        match assoc {
            Association::HasMany { name, target, foreign_key, .. } => {
                methods.push(synth_has_many_reader(owner, name, target, foreign_key));
            }
            Association::BelongsTo { name, target, foreign_key, .. } => {
                methods.push(synth_belongs_to_reader(owner, name, target, foreign_key));
            }
            // has_one and HABTM land when a fixture demands them.
            _ => {}
        }
    }
}

fn synth_has_many_reader(
    owner: &ClassId,
    name: &Symbol,
    target: &ClassId,
    foreign_key: &Symbol,
) -> MethodDef {
    // def comments; Comment.where(article_id: @id); end
    let where_args = vec![Expr::new(
        Span::synthetic(),
        ExprNode::Hash {
            entries: vec![(
                lit_sym(foreign_key.clone()),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Ivar { name: Symbol::from("id") },
                ),
            )],
            braced: false,
        },
    )];

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(target)),
            method: Symbol::from("where"),
            args: where_args,
            block: None,
            parenthesized: true,
        },
    );

    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

fn synth_belongs_to_reader(
    owner: &ClassId,
    name: &Symbol,
    target: &ClassId,
    foreign_key: &Symbol,
) -> MethodDef {
    // def article
    //   @article_id == 0 ? nil : Article.find_by(id: @article_id)
    // end
    let cond = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: foreign_key.clone() },
            )),
            method: Symbol::from("=="),
            args: vec![lit_int(0)],
            block: None,
            parenthesized: false,
        },
    );

    let find_by = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(target)),
            method: Symbol::from("find_by"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Hash {
                    entries: vec![(
                        lit_sym(Symbol::from("id")),
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::Ivar { name: foreign_key.clone() },
                        ),
                    )],
                    braced: false,
                },
            )],
            block: None,
            parenthesized: true,
        },
    );

    let body = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: nil_lit(),
            else_branch: find_by,
        },
    );

    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
}

/// `has_many :children, dependent: :destroy` lowers to a `before_destroy`
/// callback cascading `destroy` over each child. Multiple dependent
/// has_manys collapse into one `before_destroy` since Ruby allows only
/// one `def` per name — they fold into a single body in source order.
fn push_dependent_destroy(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for assoc in model.associations() {
        if let Association::HasMany { name, dependent, .. } = assoc {
            if matches!(dependent, Dependent::Destroy) {
                // assoc_name.each { |c| c.destroy }
                let iter_body = Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(var_ref(Symbol::from("c"))),
                        method: Symbol::from("destroy"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                );
                let block = Expr::new(
                    Span::synthetic(),
                    ExprNode::Lambda {
                        params: vec![Symbol::from("c")],
                        block_param: None,
                        body: iter_body,
                        block_style: crate::expr::BlockStyle::Brace,
                    },
                );
                stmts.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            Span::synthetic(),
                            ExprNode::Send {
                                recv: None,
                                method: name.clone(),
                                args: Vec::new(),
                                block: None,
                                parenthesized: false,
                            },
                        )),
                        method: Symbol::from("each"),
                        args: Vec::new(),
                        block: Some(block),
                        parenthesized: false,
                    },
                ));
            }
        }
    }

    if stmts.is_empty() {
        return;
    }

    methods.push(MethodDef {
        name: Symbol::from("before_destroy"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: seq(stmts),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
    });
}

// ---------------------------------------------------------------------------
// Unknown body items recognized as Rails markers. Most Unknowns stay
// dropped (they're emitter responsibility or future-lowerer work), but
// a small set carry semantics that translate cleanly into method
// definitions on the lowered class.
// ---------------------------------------------------------------------------

/// `primary_abstract_class` marks a model as the abstract base of a Rails
/// app. Lowered to `def self.abstract?; true; end` — the explicit form
/// spinel-blog's runtime expects.
fn push_unknown_marker_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        if let ModelBodyItem::Unknown { expr, .. } = item {
            if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
                if args.is_empty() && method.as_str() == "primary_abstract_class" {
                    methods.push(MethodDef {
                        name: Symbol::from("abstract?"),
                        receiver: MethodReceiver::Class,
                        params: Vec::new(),
                        body: Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Bool { value: true } },
                        ),
                        signature: None,
                        effects: EffectSet::default(),
                        enclosing_class: Some(model.name.0.clone()),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// broadcasts_to expansion: one DSL line synthesizes three lifecycle
// methods (after_create_commit / after_update_commit /
// after_destroy_commit), each calling `Broadcasts.<action>(stream:,
// target:, html:)`. The lambda-form channel's param (e.g. `comment`
// in `->(comment) { "article_#{comment.article_id}_comments" }`)
// gets rewritten to ivar / self references so the expanded body
// reads from the model's own state.
//
// Convention (mirrors Rails turbo + spinel-blog reference):
//   - create: action = inserts_by (default :append). target = explicit
//     `target:` override OR the channel string (when literal). html =
//     `Views::<Plural>.<singular>(self)`.
//   - update: action = :replace. target = "<class_singular>_#{@id}".
//     html = `Views::<Plural>.<singular>(self)`.
//   - destroy: action = :remove. target = "<class_singular>_#{@id}".
//     no html (remove takes no payload).
// ---------------------------------------------------------------------------

fn push_broadcasts_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "broadcasts_to" {
            continue;
        }
        if args.is_empty() {
            continue;
        }

        let (channel_expr, self_param) = match &*args[0].node {
            ExprNode::Lambda { body, params, .. } => (body.clone(), params.first().cloned()),
            ExprNode::Lit { value: Literal::Str { .. } } => (args[0].clone(), None),
            _ => continue,
        };

        let mut create_action = BroadcastAct::Append;
        let mut create_target_override: Option<Expr> = None;
        if let Some(opts) = args.get(1) {
            if let ExprNode::Hash { entries, .. } = &*opts.node {
                for (k, v) in entries {
                    let Some(key) = sym_key(k) else { continue };
                    match key.as_str() {
                        "inserts_by" => {
                            if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                                create_action = match value.as_str() {
                                    "prepend" => BroadcastAct::Prepend,
                                    "replace" => BroadcastAct::Replace,
                                    "append" => BroadcastAct::Append,
                                    _ => BroadcastAct::Append,
                                };
                            }
                        }
                        "target" => create_target_override = Some(v.clone()),
                        _ => {}
                    }
                }
            }
        }

        let stream_expr = rewrite_lambda_param(&channel_expr, self_param.as_ref());
        let create_target = create_target_override
            .map(|t| rewrite_lambda_param(&t, self_param.as_ref()))
            .unwrap_or_else(|| stream_expr.clone());
        let canonical_target = canonical_record_target(&model.name);
        let html_partial = views_render_self(&model.name);

        let create_call = broadcasts_call(
            create_action,
            stream_expr.clone(),
            create_target,
            Some(html_partial.clone()),
        );
        let update_call = broadcasts_call(
            BroadcastAct::Replace,
            stream_expr.clone(),
            canonical_target.clone(),
            Some(html_partial),
        );
        let destroy_call = broadcasts_call(
            BroadcastAct::Remove,
            stream_expr,
            canonical_target,
            None,
        );

        fold_into_or_push(methods, model, "after_create_commit", create_call);
        fold_into_or_push(methods, model, "after_update_commit", update_call);
        fold_into_or_push(methods, model, "after_destroy_commit", destroy_call);
    }
}

#[derive(Clone, Copy)]
enum BroadcastAct {
    Append,
    Prepend,
    Replace,
    Remove,
}

impl BroadcastAct {
    fn method_name(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Prepend => "prepend",
            Self::Replace => "replace",
            Self::Remove => "remove",
        }
    }
}

fn broadcasts_call(
    action: BroadcastAct,
    stream: Expr,
    target: Expr,
    html: Option<Expr>,
) -> Expr {
    let mut entries: Vec<(Expr, Expr)> = vec![
        (lit_sym(Symbol::from("stream")), stream),
        (lit_sym(Symbol::from("target")), target),
    ];
    if let Some(h) = html {
        entries.push((lit_sym(Symbol::from("html")), h));
    }
    let kwargs = Expr::new(Span::synthetic(), ExprNode::Hash { entries, braced: false });
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("Broadcasts")] },
            )),
            method: Symbol::from(action.method_name()),
            args: vec![kwargs],
            block: None,
            parenthesized: true,
        },
    )
}

/// `"<class_singular>_#{@id}"` — the canonical per-record DOM target
/// Rails turbo uses on update + destroy regardless of `target:` option.
fn canonical_record_target(class_name: &ClassId) -> Expr {
    let singular = crate::naming::snake_case(class_name.0.as_str());
    Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Text { value: format!("{singular}_") },
                crate::expr::InterpPart::Expr {
                    expr: Expr::new(
                        Span::synthetic(),
                        ExprNode::Ivar { name: Symbol::from("id") },
                    ),
                },
            ],
        },
    )
}

/// `Views::<Plural>.<singular>(self)` — the partial-render call used
/// for the `html:` payload on create/update broadcasts.
fn views_render_self(class_name: &ClassId) -> Expr {
    let plural = crate::naming::pluralize_snake(class_name.0.as_str());
    let plural_camel = camelize(&plural);
    let singular = crate::naming::snake_case(class_name.0.as_str());
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const {
                    path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
                },
            )),
            method: Symbol::from(singular),
            args: vec![self_ref()],
            block: None,
            parenthesized: true,
        },
    )
}

fn camelize(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = true;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Look up an existing `Method` named `hook_name` and append `call` to
/// its body's Seq, OR push a new method with `call` as the body. The
/// fold preserves source order; broadcasts_to runs first so its calls
/// lead any block-form callback bodies that the next pass would add.
fn fold_into_or_push(methods: &mut Vec<MethodDef>, model: &Model, hook_name: &str, call: Expr) {
    let hook = Symbol::from(hook_name);
    if let Some(existing) = methods.iter_mut().find(|m| m.name == hook) {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.push(call);
        existing.body = seq(stmts);
    } else {
        methods.push(MethodDef {
            name: hook,
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: call,
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
        });
    }
}

/// Rewrite `param.attr` → `@attr` and bare `param` → `self`. The
/// channel/target lambda's parameter refers to the record being
/// broadcast; in the expanded method body those references resolve
/// to the model's own state.
fn rewrite_lambda_param(e: &Expr, param: Option<&Symbol>) -> Expr {
    let Some(p) = param else { return e.clone() };
    let new_node = match &*e.node {
        ExprNode::Var { name, .. } if name == p => ExprNode::SelfRef,
        ExprNode::Send { recv: Some(r), method, args, block, parenthesized } => {
            // `param.attr` (no args, no block) → `@attr`.
            if let ExprNode::Var { name, .. } = &*r.node {
                if name == p && args.is_empty() && block.is_none() {
                    return Expr::new(
                        Span::synthetic(),
                        ExprNode::Ivar { name: method.clone() },
                    );
                }
            }
            ExprNode::Send {
                recv: Some(rewrite_lambda_param(r, Some(p))),
                method: method.clone(),
                args: args.iter().map(|a| rewrite_lambda_param(a, Some(p))).collect(),
                block: block.as_ref().map(|b| rewrite_lambda_param(b, Some(p))),
                parenthesized: *parenthesized,
            }
        }
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|part| match part {
                    crate::expr::InterpPart::Text { value } => {
                        crate::expr::InterpPart::Text { value: value.clone() }
                    }
                    crate::expr::InterpPart::Expr { expr } => crate::expr::InterpPart::Expr {
                        expr: rewrite_lambda_param(expr, Some(p)),
                    },
                })
                .collect(),
        },
        _ => return e.clone(),
    };
    Expr::new(Span::synthetic(), new_node)
}

fn sym_key(e: &Expr) -> Option<&Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Block-form lifecycle callbacks: `after_create_commit { … }` etc. They
// surface as Unknown body items (parse_callback rejects them — no
// symbol target, just a block). Lowered to a `def hook_name; <block-
// body>; end`. Multiple sources can target the same hook (block-form
// callback + broadcasts_to expansion + dependent: :destroy cascade);
// when this lowering finds an existing method with the matching name
// it folds the block body into that method's Seq, preserving source
// order across sources.
// ---------------------------------------------------------------------------

/// Lifecycle hook names that appear as block-form Unknown items. Names
/// not in this set fall through to plain Unknown (they're future
/// lowerer or emit work). Includes the `_commit` variants Rails sugar
/// adds beyond the raw `after_commit` hook in `CallbackHook`.
const BLOCK_CALLBACK_HOOKS: &[&str] = &[
    "before_validation",
    "after_validation",
    "before_save",
    "after_save",
    "before_create",
    "after_create",
    "before_update",
    "after_update",
    "before_destroy",
    "after_destroy",
    "after_commit",
    "after_rollback",
    "after_create_commit",
    "after_update_commit",
    "after_destroy_commit",
    "after_save_commit",
];

fn push_block_callback_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
            continue;
        };
        if !args.is_empty() {
            continue;
        }
        let hook = method.as_str();
        if !BLOCK_CALLBACK_HOOKS.contains(&hook) {
            continue;
        }
        let ExprNode::Lambda { body: lambda_body, .. } = &*block.node else {
            continue;
        };

        let hook_sym = method.clone();
        if let Some(existing) = methods.iter_mut().find(|m| m.name == hook_sym) {
            // Fold this block's body into the existing method, preserving
            // source order (existing body's stmts first, then this block's).
            let mut stmts = match &*existing.body.node {
                ExprNode::Seq { exprs } => exprs.clone(),
                _ => vec![existing.body.clone()],
            };
            match &*lambda_body.node {
                ExprNode::Seq { exprs } => stmts.extend(exprs.clone()),
                _ => stmts.push(lambda_body.clone()),
            }
            existing.body = seq(stmts);
        } else {
            methods.push(MethodDef {
                name: hook_sym,
                receiver: MethodReceiver::Instance,
                params: Vec::new(),
                body: lambda_body.clone(),
                signature: None,
                effects: EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Small ExprNode constructors used throughout. Each takes a synthetic span
// since lowered methods don't correspond to a single source location.
// ---------------------------------------------------------------------------

fn lit_str(s: String) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } })
}

fn lit_sym(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name } })
}

fn lit_int(value: i64) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } })
}

fn lit_float(value: f64) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } })
}

fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

fn var_ref(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name })
}

fn class_const(id: &ClassId) -> Expr {
    let path: Vec<Symbol> = id.0.as_str().split("::").map(Symbol::from).collect();
    Expr::new(Span::synthetic(), ExprNode::Const { path })
}

fn self_ref() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::SelfRef)
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

fn is_id_column(name: &Symbol) -> bool {
    let s = name.as_str();
    s == "id" || s.ends_with("_id")
}
