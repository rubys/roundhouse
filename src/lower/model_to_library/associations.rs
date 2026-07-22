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

/// Join recipe for a `has_many :through` collection writer, resolved
/// against the app's model slice.
pub(super) enum ThroughWriterJoin {
    /// Join class, owner-side fk, target-side fk — synthesize the
    /// writer. The target fk comes from the join model's `belongs_to`
    /// matching the target class when that model is in the slice
    /// (survives `foreign_key:` overrides); a join model outside the
    /// slice falls back to the `<target>_id` convention.
    Resolved(ClassId, Symbol, Symbol),
    /// The chain is nested — the join model reaches the target through
    /// ANOTHER association rather than a `belongs_to` (`Category
    /// has_many :stories, through: :tags` — Tag#stories itself goes
    /// through taggings), so there is no join row to write. Rails makes
    /// these collections read-only
    /// (HasManyThroughNestedAssociationsAreReadonly); no writer.
    Nested(ClassId),
    /// No sibling has_many names the join — nothing to synthesize.
    NoJoin,
}

/// Resolve the writer's join recipe: the sibling through association
/// names the join class and owner-side fk; the join model's
/// `belongs_to` supplies the target-side fk. Shared with the
/// initialize synthesis (schema.rs), which must skip the writer's
/// `_stale` flag when no writer will exist.
pub(super) fn through_writer_join(
    model: &Model,
    models: &[Model],
    thr_name: &Symbol,
    target: &ClassId,
) -> ThroughWriterJoin {
    let Some((join_class, owner_fk, thr_through)) = model.associations().find_map(|a| match a {
        Association::HasMany { name: n, target: jt, foreign_key: jfk, through: jthru, .. }
            if n == thr_name =>
        {
            Some((jt.clone(), jfk.clone(), jthru.clone()))
        }
        _ => None,
    }) else {
        return ThroughWriterJoin::NoJoin;
    };
    // First hop already indirect (`through:` an association that is
    // itself `:through`) — nested regardless of the join model's shape.
    if thr_through.is_some() {
        return ThroughWriterJoin::Nested(join_class);
    }
    let Some(join_model) = models.iter().find(|m| m.name == join_class) else {
        let src_fk =
            Symbol::from(format!("{}_id", crate::naming::snake_case(target.0.as_str())));
        return ThroughWriterJoin::Resolved(join_class, owner_fk, src_fk);
    };
    match join_model.associations().find_map(|a| match a {
        Association::BelongsTo { target: t, foreign_key, .. } if t == target => {
            Some(foreign_key.clone())
        }
        _ => None,
    }) {
        Some(src_fk) => ThroughWriterJoin::Resolved(join_class, owner_fk, src_fk),
        None => ThroughWriterJoin::Nested(join_class),
    }
}

pub(super) fn push_association_methods(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    models: &[Model],
) {
    let owner = &model.name;
    for (span, assoc) in model.spanned_associations() {
        let before = methods.len();
        match assoc {
            Association::HasMany { name, target, foreign_key, as_interface, scope, through, .. } => {
                methods.push(synth_has_many_reader(
                    owner,
                    name,
                    target,
                    foreign_key,
                    as_interface.as_ref(),
                    scope.as_ref(),
                ));
                methods.push(synth_preload_setter(owner, name, target));
                // `has_many :through` collection writer (`story.tags =
                // [tag]` — the factory/edit shape). Stages the target
                // collection and marks it stale; `_sync_<name>` folds
                // into after_save (before any user callbacks — they
                // run against synced join rows) and replaces the join
                // rows there. Deferred-sync is an honest subset of
                // Rails, which syncs immediately for persisted owners.
                // The sibling through association names the join class
                // and the owner-side fk; the join model's `belongs_to`
                // matching the target supplies the target-side fk (see
                // `through_writer_join`). A nested chain gets no writer
                // — Rails raises
                // HasManyThroughNestedAssociationsAreReadonly on
                // assignment, so a missing writer (NoMethodError /
                // compile refusal) is the honest equivalent; the skip
                // is ledgered as lower_residue.
                if let Some(thr_name) = through {
                    let writer_name = Symbol::from(format!("{}=", name.as_str()));
                    match through_writer_join(model, models, thr_name, target) {
                        ThroughWriterJoin::Resolved(join_class, owner_fk, src_fk) => {
                            if !model_defines_instance_method(model, &writer_name)
                                && !methods.iter().any(|m| {
                                    m.name == writer_name && m.receiver == MethodReceiver::Instance
                                })
                            {
                                methods.push(synth_through_collection_writer(owner, name, target));
                                methods.push(synth_through_sync(
                                    owner,
                                    name,
                                    &join_class,
                                    &owner_fk,
                                    &src_fk,
                                ));
                                super::markers::fold_into_or_push(
                                    methods,
                                    model,
                                    "after_save",
                                    Expr::new(
                                        Span::synthetic(),
                                        ExprNode::Send {
                                            recv: None,
                                            method: Symbol::from(format!(
                                                "_sync_{}",
                                                name.as_str()
                                            )),
                                            args: vec![],
                                            block: None,
                                            parenthesized: false,
                                        },
                                    ),
                                );
                            }
                        }
                        ThroughWriterJoin::Nested(join_class) => {
                            let kind = crate::diagnostic::DiagnosticKind::LowerResidue {
                                pass: Symbol::from("through_writer"),
                                construct: Symbol::from("has_many"),
                                reason: Symbol::from("nested through"),
                            };
                            let d = crate::diagnostic::Diagnostic {
                                span: model.span,
                                severity: crate::diagnostic::Diagnostic::default_severity(&kind),
                                kind,
                                message: format!(
                                    "`{owner}#{name}=` not synthesized: `has_many :{name}, \
                                     through: :{thr}` is nested — `{join}` reaches `{target}` \
                                     through another association, not a `belongs_to` — and \
                                     Rails makes nested through collections read-only \
                                     (HasManyThroughNestedAssociationsAreReadonly raises on \
                                     assignment)",
                                    owner = owner.0.as_str(),
                                    name = name.as_str(),
                                    thr = thr_name.as_str(),
                                    join = join_class.0.as_str(),
                                    target = target.0.as_str(),
                                ),
                            };
                            crate::emit::diagnostics::push(d);
                        }
                        ThroughWriterJoin::NoJoin => {}
                    }
                }
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
    scope: Option<&Expr>,
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
    // Association scope (`has_many :comments, -> { order(created_at:
    // :desc) }` / Sequel's `order:` option) — graft the recorded chain
    // onto the FK query so the reader honors it: re-root the scope
    // expression's leftmost implicit-self Send onto `lazy_query`,
    // yielding `Comment.where(fk: @id).order(created_at: :desc)`. The
    // arel fold then carries the ORDER BY into the compiled SQL (or the
    // chain falls back to the runtime Relation, which evaluates it).
    // A scope whose root isn't an implicit-self call chain is left
    // ungrafted — the previous (scope-ignoring) behavior, never a
    // corrupted query. NOTE: the eager-load path (`includes` →
    // `_preload_comments`) does not apply scopes yet; readers cover
    // the per-record access pattern (show pages), which is where
    // ordering is user-visible today.
    let lazy_query = match scope {
        Some(scope_expr) => graft_scope(scope_expr, lazy_query),
        None => lazy_query,
    };

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

/// Re-root an association-scope chain onto `base`: walk the scope's
/// Send spine to its leftmost implicit-self call and substitute `base`
/// as that call's receiver. Returns `base` unchanged when the scope's
/// root isn't an implicit-self Send (a shape the graft can't express —
/// better the unscoped query than a mangled one; the gap stays visible
/// as a behavioral diff, not a corrupt emit).
fn graft_scope(scope: &Expr, base: Expr) -> Expr {
    fn reroot(e: &Expr, base: Expr) -> Option<Expr> {
        let ExprNode::Send { recv, method, args, block, parenthesized } = &*e.node else {
            return None;
        };
        let new_recv = match recv {
            None => base,
            Some(inner) => reroot(inner, base)?,
        };
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(new_recv),
                method: method.clone(),
                args: args.clone(),
                block: block.clone(),
                parenthesized: *parenthesized,
            },
        ))
    }
    match reroot(scope, base.clone()) {
        Some(grafted) => grafted,
        None => base,
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
/// `def tags=(values)` — stage the target collection for a `has_many
/// :through` association and mark the join rows stale:
///
///   def tags=(values)
///     @tags_cache = values
///     @tags_loaded = true
///     @tags_stale = true
///   end
///
/// The cache/loaded pair is the same one the reader and
/// `_preload_<name>` use, so a read-after-write returns the assigned
/// collection without touching the DB (Rails: the writer marks the
/// target loaded). `_sync_<name>` consumes the stale flag at save.
fn synth_through_collection_writer(owner: &ClassId, name: &Symbol, target: &ClassId) -> MethodDef {
    let values = Symbol::from("values");
    let values_ty = Ty::Array { elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }) };
    let ivar_assign = |ivar: String, value: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign { target: LValue::Ivar { name: Symbol::from(ivar) }, value },
        )
    };
    let bool_lit = |value: bool| {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value } })
    };
    let body = seq(vec![
        ivar_assign(format!("{}_cache", name.as_str()), var_ref(values.clone())),
        ivar_assign(format!("{}_loaded", name.as_str()), bool_lit(true)),
        ivar_assign(format!("{}_stale", name.as_str()), bool_lit(true)),
    ]);
    MethodDef {
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(values.clone())],
        body,
        signature: Some(fn_sig(vec![(values, values_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

/// `def _sync_tags` — replace the join rows with the staged
/// collection, once, at save time (folded into after_save):
///
///   def _sync_tags
///     if @tags_stale
///       @tags_stale = false
///       Tagging.where(story_id: @id).each { |__row| __row.destroy }
///       @tags_cache.each do |__target|
///         __join = Tagging.new
///         __join.story_id = @id
///         __join.tag_id = __target.id
///         __join.save
///       end
///     end
///   end
///
/// Replace-all rather than a diff: unchanged pairs get fresh join
/// rows (new ids), which no corpus spec observes; Rails diffs, and
/// deletes without callbacks — `destroy` here keeps any synthesized
/// join-model cascades honest.
fn synth_through_sync(
    owner: &ClassId,
    name: &Symbol,
    join_class: &ClassId,
    owner_fk: &Symbol,
    src_fk: &Symbol,
) -> MethodDef {
    use crate::ident::VarId;

    let stale_ivar = Symbol::from(format!("{}_stale", name.as_str()));
    let id_ivar = || Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") });
    let send = |recv: Expr, method: &str, args: Vec<Expr>| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    };

    // Tagging.where(story_id: @id).each { |__row| __row.destroy }
    let where_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(join_class)),
            method: Symbol::from("where"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Hash {
                    entries: vec![(lit_sym(owner_fk.clone()), id_ivar())],
                    kwargs: true,
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let row = Symbol::from("__row");
    let delete_block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![row.clone()],
            block_param: None,
            body: send(var_ref(row), "destroy", vec![]),
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    let delete_all = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(where_call),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(delete_block),
            parenthesized: false,
        },
    );

    // @tags_cache.each { |__target| __join = Join.new; __join.<owner_fk> = @id;
    //                    __join.<src_fk> = __target.id; __join.save }
    let target_var = Symbol::from("__target");
    let join_var = Symbol::from("__join");
    let insert_body = seq(vec![
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: join_var.clone() },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(class_const(join_class)),
                        method: Symbol::from("new"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    },
                ),
            },
        ),
        send(var_ref(join_var.clone()), &format!("{}=", owner_fk.as_str()), vec![id_ivar()]),
        send(
            var_ref(join_var.clone()),
            &format!("{}=", src_fk.as_str()),
            vec![send(var_ref(target_var.clone()), "id", vec![])],
        ),
        send(var_ref(join_var), "save", vec![]),
    ]);
    let insert_block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![target_var],
            block_param: None,
            body: insert_body,
            block_style: crate::expr::BlockStyle::Do,
        },
    );
    let insert_all = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) },
            )),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(insert_block),
            parenthesized: false,
        },
    );

    let clear_stale = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: stale_ivar.clone() },
            value: Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Bool { value: false } },
            ),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(Span::synthetic(), ExprNode::Ivar { name: stale_ivar }),
            then_branch: seq(vec![clear_stale, delete_all, insert_all]),
            else_branch: nil_lit(),
        },
    );
    MethodDef {
        name: Symbol::from(format!("_sync_{}", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

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
