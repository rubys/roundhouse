//! Associations: has_many becomes a typed reader returning a where-style
//! query. dependent: :destroy generates a `before_destroy` cascade that
//! iterates and destroys each child.

use crate::dialect::{
    AccessorKind, Association, Dependent, MethodDef, MethodReceiver, Model, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::{class_const, fn_sig, lit_int, lit_sym, nil_lit, seq, var_ref};

pub(super) fn push_association_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let owner = &model.name;
    for (span, assoc) in model.spanned_associations() {
        let before = methods.len();
        match assoc {
            Association::HasMany { name, target, foreign_key, as_interface, .. } => {
                methods.push(synth_has_many_reader(
                    owner,
                    name,
                    target,
                    foreign_key,
                    as_interface.as_ref(),
                ));
                methods.push(synth_preload_setter(owner, name, target));
            }
            Association::BelongsTo {
                name,
                target,
                foreign_key,
                polymorphic: true,
                polymorphic_targets,
                ..
            } if !polymorphic_targets.is_empty() => {
                // Polymorphic: the reader dispatches on the `<name>_type`
                // column across the resolved implementor set; the writer
                // stores both halves of the (type, id) pair.
                methods.push(synth_polymorphic_reader(
                    owner,
                    name,
                    polymorphic_targets,
                    foreign_key,
                ));
                let writer_name = Symbol::from(format!("{}=", name.as_str()));
                if !model_defines_instance_method(model, &writer_name)
                    && !methods
                        .iter()
                        .any(|m| m.name == writer_name && m.receiver == MethodReceiver::Instance)
                {
                    methods.push(synth_polymorphic_writer(
                        owner,
                        name,
                        polymorphic_targets,
                        foreign_key,
                    ));
                }
            }
            Association::BelongsTo { name, target, foreign_key, .. } => {
                methods.push(synth_belongs_to_reader(owner, name, target, foreign_key));
                // Rails provides the writer alongside the reader
                // (`comment.story = obj` stores the foreign key). A
                // custom writer in the model body must win (Rails: the
                // later `def` overrides the association's), but
                // `push_user_methods` runs after this and drops
                // collisions — so the synthesized writer yields here.
                // Same for a name an earlier synthesizer claimed (a
                // column sharing the association's name).
                let writer_name = Symbol::from(format!("{}=", name.as_str()));
                if !model_defines_instance_method(model, &writer_name)
                    && !methods
                        .iter()
                        .any(|m| m.name == writer_name && m.receiver == MethodReceiver::Instance)
                {
                    methods.push(synth_belongs_to_writer(owner, name, target, foreign_key));
                }
            }
            Association::HasOne { name, target, foreign_key, as_interface, .. } => {
                methods.push(synth_has_one_reader(
                    owner,
                    name,
                    target,
                    foreign_key,
                    as_interface.as_ref(),
                ));
            }
            // HABTM lands when a fixture demands it.
            _ => {}
        }
        // Every method this declaration synthesized attributes to the
        // `has_many`/`belongs_to` line it came from.
        for m in &mut methods[before..] {
            m.body.inherit_span(span);
        }
    }
}

fn synth_has_many_reader(
    owner: &ClassId,
    name: &Symbol,
    target: &ClassId,
    foreign_key: &Symbol,
    as_interface: Option<&Symbol>,
) -> MethodDef {
    // def comments; Comment.where(article_id: @id); end
    //
    // With `as: :notifiable` the rows point back through the
    // polymorphic interface columns, so the type half scopes too:
    //   Notification.where(notifiable_id: @id, notifiable_type: "Comment")
    let mut entries = vec![(
        lit_sym(foreign_key.clone()),
        Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("id") },
        ),
    )];
    if let Some(intf) = as_interface {
        entries.push((
            lit_sym(Symbol::from(format!("{intf}_type"))),
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: Literal::Str { value: owner.0.as_str().to_string() },
                },
            ),
        ));
    }
    let where_args = vec![Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries, kwargs: true },
    )];

    let lazy_query = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(target)),
            method: Symbol::from("where"),
            args: where_args,
            block: None,
            parenthesized: true,
        },
    );

    // Cache-aware body (issue #27):
    //   def comments
    //     return @comments_cache if @comments_loaded   # eager-loaded
    //     @comments_cache = Comment.where(article_id: @id)  # lazy + memoize
    //     @comments_loaded = true
    //     @comments_cache
    //   end
    // The lazy fallback MUST stay — paths like `render @article.comments`
    // (show.html.erb) reach the reader with no `includes` upstream, so
    // `@comments_loaded` is unset (false) and the query runs. When a
    // controller's `includes(:comments)` preload ran, the setter
    // `_preload_comments` flipped the flag and the guard short-circuits.
    //
    // Pure-read guard form (no memoize):
    //   return @comments_cache if @comments_loaded
    //   Comment.where(article_id: @id)
    // Crucially the reader does NOT write any ivar, so it stays a read-
    // only method — Rust emits `&self` and the read-only callers (views
    // iterating `@articles` and calling `article.comments()`) borrow
    // immutably. A memoizing variant (`@cache = …` in the reader) would
    // force `&mut self` and break every immutable caller. The guard's
    // early `return @cache` matches the belongs_to reader shape, which
    // every target already compiles; the lazy query stays at statement
    // level so TS doesn't ternary-ize a multi-statement branch.
    let guard = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(Span::synthetic(), ExprNode::Ivar { name: loaded_ivar(name) }),
            then_branch: Expr::new(
                Span::synthetic(),
                ExprNode::Return {
                    value: Expr::new(Span::synthetic(), ExprNode::Ivar { name: cache_ivar(name) }),
                },
            ),
            else_branch: nil_lit(),
        },
    );
    let body = seq(vec![guard, lazy_query]);

    // has_many reader — body computes (`Comment.where(...)`), so it
    // must remain a Method even though Ruby's `article.comments` reads
    // like an attribute. Marking AttributeReader would cause the TS
    // emitter to drop the body and emit a bare field, which would be
    // assigned undefined at construction.
    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(
            vec![],
            Ty::Array { elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }) },
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// has_one reader — the has_many query narrowed to one row:
/// `def moderation; Moderation.where(comment_id: @id).first; end`
/// (lobsters `Comment has_one :moderation`, read by gone_text). No
/// preload cache — has_one reads are rare enough that the lazy query
/// is the whole story until an includes() fixture demands more.
fn synth_has_one_reader(
    owner: &ClassId,
    name: &Symbol,
    target: &ClassId,
    foreign_key: &Symbol,
    as_interface: Option<&Symbol>,
) -> MethodDef {
    let mut entries = vec![(
        lit_sym(foreign_key.clone()),
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") }),
    )];
    // See `synth_has_many_reader` — `as:` adds the type-half scope.
    if let Some(intf) = as_interface {
        entries.push((
            lit_sym(Symbol::from(format!("{intf}_type"))),
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: Literal::Str { value: owner.0.as_str().to_string() },
                },
            ),
        ));
    }
    let where_args = vec![Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries, kwargs: true },
    )];
    let query = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(target)),
            method: Symbol::from("where"),
            args: where_args,
            block: None,
            parenthesized: true,
        },
    );
    let first = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(query),
            method: Symbol::from("first"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: first,
        signature: Some(fn_sig(
            vec![],
            Ty::Union {
                variants: vec![Ty::Class { id: target.clone(), args: vec![] }, Ty::Nil],
            },
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// Cache + loaded-flag ivar names for a has_many association. Kept in
/// one place so the reader (which reads them) and the setter (which
/// writes them) can't drift.
fn cache_ivar(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_cache", name.as_str()))
}
fn loaded_ivar(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_loaded", name.as_str()))
}

/// Body-typer ivar bindings for a model's has_many eager-load caches
/// (issue #27): `@<assoc>_cache` is `Array<Target>`, `@<assoc>_loaded`
/// is `Bool`. The reader reads both and the constructor
/// (model_to_library::schema) initializes them to `[]` / `false`, but
/// they aren't schema columns, so the per-method typer must be seeded
/// with them explicitly or the reads stay `Var(0)` (the strict-0
/// untyped residual the lowered_real_blog_typing_residual gate counts).
///
/// Non-nilable on purpose: the constructor always initializes them, and
/// the reader's `return @<assoc>_cache` must match its `Array<Target>`
/// signature — a Nil union would reintroduce the Crystal "returning
/// (Array(Comment)|Nil)" mismatch the eager-load fan-out already closed.
/// Shares `cache_ivar`/`loaded_ivar` with the synthesizers so the names
/// can't drift.
pub(in crate::lower::model_to_library) fn assoc_cache_ivar_bindings(
    model: &Model,
) -> Vec<(Symbol, Ty)> {
    let mut out = Vec::new();
    for assoc in model.associations() {
        if let Association::HasMany { name, target, .. } = assoc {
            out.push((
                cache_ivar(name),
                Ty::Array { elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }) },
            ));
            out.push((loaded_ivar(name), Ty::Bool));
        }
    }
    out
}
fn lit_bool(value: bool) -> Expr {
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value } });
    e.ty = Some(Ty::Bool);
    e
}

/// `def _preload_comments(list); @comments_cache = list;
/// @comments_loaded = true; end` — the controller's eager-load
/// distribute loop calls this per parent to seed the cache. Owning the
/// ivar writes here (rather than in the controller IR) keeps the cache
/// representation encapsulated in the model lowerer; the only contract
/// the controller side depends on is the `_preload_<assoc>` method name.
fn synth_preload_setter(owner: &ClassId, name: &Symbol, target: &ClassId) -> MethodDef {
    let list = Symbol::from("list");
    let list_ty = Ty::Array { elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }) };

    let body = seq(vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: cache_ivar(name) },
                value: var_ref(list.clone()),
            },
        ),
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: loaded_ivar(name) },
                value: lit_bool(true),
            },
        ),
    ]);

    MethodDef {
        name: Symbol::from(format!("_preload_{}", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(list.clone())],
        body,
        signature: Some(fn_sig(vec![(list, list_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
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
                    kwargs: true,
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

    // belongs_to reader — same reasoning as has_many: body computes
    // (`Article.find_by(...)`), Method not AttributeReader.
    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(
            vec![],
            Ty::Union {
                variants: vec![
                    Ty::Class { id: target.clone(), args: vec![] },
                    Ty::Nil,
                ],
            },
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

fn synth_polymorphic_reader(
    owner: &ClassId,
    name: &Symbol,
    targets: &[ClassId],
    foreign_key: &Symbol,
) -> MethodDef {
    // def notifiable
    //   case @notifiable_type
    //   when "Comment" then Comment.find_by(id: @notifiable_id)
    //   when "Message" then Message.find_by(id: @notifiable_id)
    //   else nil
    //   end
    // end
    //
    // Rails stores the implementor's class name in `<name>_type`; the
    // target set was resolved at ingest from the inverse `as:` decls.
    let type_col = Symbol::from(format!("{}_type", name.as_str()));
    let find_by = |t: &ClassId| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(class_const(t)),
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
                        kwargs: true,
                    },
                )],
                block: None,
                parenthesized: true,
            },
        )
    };
    let mut arms: Vec<crate::expr::Arm> = targets
        .iter()
        .map(|t| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Str { value: t.0.as_str().to_string() },
            },
            guard: None,
            body: find_by(t),
        })
        .collect();
    arms.push(crate::expr::Arm {
        pattern: crate::expr::Pattern::Wildcard,
        guard: None,
        body: nil_lit(),
    });
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Case {
            scrutinee: Expr::new(Span::synthetic(), ExprNode::Ivar { name: type_col }),
            arms,
        },
    );

    let mut variants: Vec<Ty> = targets
        .iter()
        .map(|t| Ty::Class { id: t.clone(), args: vec![] })
        .collect();
    variants.push(Ty::Nil);
    MethodDef {
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Union { variants })),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn synth_polymorphic_writer(
    owner: &ClassId,
    name: &Symbol,
    targets: &[ClassId],
    foreign_key: &Symbol,
) -> MethodDef {
    // def notifiable=(value)
    //   if value.nil?
    //     @notifiable_id = 0
    //     @notifiable_type = ""
    //   else
    //     @notifiable_id = value.id
    //     case value
    //     when Comment then @notifiable_type = "Comment"
    //     when Message then @notifiable_type = "Message"
    //     end
    //   end
    // end
    //
    // Both halves of the (type, id) pair, mirroring the plain writer's
    // `@fk == 0` nil sentinel. The class-pattern `when` keeps the type
    // string a compile-time constant per arm (no `.class.name`).
    let value = Symbol::from("value");
    let type_col = Symbol::from(format!("{}_type", name.as_str()));
    let assign = |target_ivar: &Symbol, v: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: target_ivar.clone() },
                value: v,
            },
        )
    };
    let lit_str = |s: &str| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: s.to_string() } },
        )
    };

    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(value.clone())),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let id_read = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(value.clone())),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let type_arms: Vec<crate::expr::Arm> = targets
        .iter()
        .map(|t| crate::expr::Arm {
            pattern: crate::expr::Pattern::Expr { expr: class_const(t) },
            guard: None,
            body: assign(&type_col, lit_str(t.0.as_str())),
        })
        .collect();
    let type_switch = Expr::new(
        Span::synthetic(),
        ExprNode::Case { scrutinee: var_ref(value.clone()), arms: type_arms },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: nil_check,
            then_branch: seq(vec![
                assign(foreign_key, lit_int(0)),
                assign(&type_col, lit_str("")),
            ]),
            else_branch: seq(vec![assign(foreign_key, id_read), type_switch]),
        },
    );

    let mut variants: Vec<Ty> = targets
        .iter()
        .map(|t| Ty::Class { id: t.clone(), args: vec![] })
        .collect();
    variants.push(Ty::Nil);
    MethodDef {
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value.clone())],
        body,
        signature: Some(fn_sig(vec![(value, Ty::Union { variants })], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

/// True when the model's own body defines an instance method `name` —
/// the signal that a synthesized association accessor must yield to
/// the app's definition.
pub(crate) fn model_defines_instance_method(model: &Model, name: &Symbol) -> bool {
    use crate::dialect::ModelBodyItem;
    model.body.iter().any(|item| {
        matches!(item, ModelBodyItem::Method { method, .. }
            if method.name == *name && method.receiver == MethodReceiver::Instance)
    })
}

fn synth_belongs_to_writer(
    owner: &ClassId,
    name: &Symbol,
    target: &ClassId,
    foreign_key: &Symbol,
) -> MethodDef {
    // def story=(value)
    //   if value.nil?
    //     @story_id = 0
    //   else
    //     @story_id = value.id
    //   end
    // end
    //
    // Stores the foreign key, mirroring the reader's `@fk == 0` nil
    // sentinel (fk columns are non-nullable Int in the schema typing).
    // No object cache: the reader re-queries by fk, which matches its
    // existing shape — assigning an UNSAVED record then reading the
    // association back is the one Rails behavior this doesn't cover.
    let value = Symbol::from("value");
    let fk_assign = |v: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: foreign_key.clone() },
                value: v,
            },
        )
    };
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(value.clone())),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let id_read = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(value.clone())),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: nil_check,
            then_branch: fk_assign(lit_int(0)),
            else_branch: fk_assign(id_read),
        },
    );

    // Void return (Ty::Nil), matching `synth_preload_setter`: callers
    // assign for the side effect, and a void shape keeps the strict
    // targets from having to thread the assign's value out of an
    // if/else statement position.
    let value_ty = Ty::Union {
        variants: vec![Ty::Class { id: target.clone(), args: vec![] }, Ty::Nil],
    };
    MethodDef {
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value.clone())],
        body,
        signature: Some(fn_sig(vec![(value, value_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

/// `has_many :children, dependent: :destroy` lowers to a `before_destroy`
/// callback cascading `destroy` over each child. Multiple dependent
/// has_manys collapse into one `before_destroy` since Ruby allows only
/// one `def` per name — they fold into a single body in source order.
pub(super) fn push_dependent_destroy(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for (span, assoc) in model.spanned_associations() {
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
                let mut cascade = Expr::new(
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
                );
                // Each cascade attributes to its `dependent: :destroy`
                // declaration.
                cascade.inherit_span(span);
                stmts.push(cascade);
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
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    });
}
