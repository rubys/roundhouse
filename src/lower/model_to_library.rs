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
    Association, Dependent, LibraryClass, MethodDef, MethodReceiver, Model,
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

    push_association_methods(&mut methods, model);
    push_dependent_destroy(&mut methods, model);

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
        params: vec![value_param],
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
        params: vec![row],
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

    MethodDef {
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params: vec![attrs],
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
        params: vec![name],
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
        params: vec![name, value],
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
        params: vec![attrs],
        body: seq(stmts),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    }
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
