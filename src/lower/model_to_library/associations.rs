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
    for assoc in model.associations() {
        match assoc {
            Association::HasMany { name, target, foreign_key, .. } => {
                methods.push(synth_has_many_reader(owner, name, target, foreign_key));
                methods.push(synth_preload_setter(owner, name, target));
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
            kwargs: true,
        },
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
    //     Comment.where(article_id: @id)               # lazy fallback
    //   end
    // The lazy fallback MUST stay — paths like `render @article.comments`
    // (show.html.erb) reach the reader with no `includes` upstream, so
    // `@comments_loaded` is unset (nil/false) and the query runs. When
    // a controller's `includes(:comments)` preload ran, the setter
    // `_preload_comments` flipped the flag and the cache short-circuits.
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: loaded_ivar(name) },
            ),
            then_branch: Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: cache_ivar(name) },
            ),
            else_branch: lazy_query,
        },
    );

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

/// Cache + loaded-flag ivar names for a has_many association. Kept in
/// one place so the reader (which reads them) and the setter (which
/// writes them) can't drift.
fn cache_ivar(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_cache", name.as_str()))
}
fn loaded_ivar(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_loaded", name.as_str()))
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
                value: {
                    let mut e = Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Bool { value: true } },
                    );
                    e.ty = Some(Ty::Bool);
                    e
                },
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

/// `has_many :children, dependent: :destroy` lowers to a `before_destroy`
/// callback cascading `destroy` over each child. Multiple dependent
/// has_manys collapse into one `before_destroy` since Ruby allows only
/// one `def` per name — they fold into a single body in source order.
pub(super) fn push_dependent_destroy(methods: &mut Vec<MethodDef>, model: &Model) {
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
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    });
}
