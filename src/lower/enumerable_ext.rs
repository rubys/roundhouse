//! ActiveSupport's core_ext reopens, grounded to a runtime function
//! instead of a reopen.
//!
//! Rails ships `index_by`, `many?`, `to_sentence` and `sole` on
//! `Enumerable`/`Array`, and `squish` on `String`, by reopening the
//! builtin — which is a shape
//! only the CRuby overlay can host: the transpiled runtimes cannot
//! reopen a builtin, and spinel AOT cannot dispatch a user-defined
//! method on one. The runtime already answers `index_by` on
//! `ActiveRecord::Relation` (relation.rb), and the twin for everything
//! else is one module function taking the collection as an argument —
//! the same rule `active_support_ext.rb` states for `blank?`: the
//! receiver is evaluated exactly once, so a receiver with effects
//! grounds too, and no `respond_to?` is needed.
//!
//! campfire builds `Sound::INDEX = BUILTIN.index_by(&:name)` in a CLASS
//! BODY, so an ungrounded call is not a late NoMethodError on some
//! route — it fires while `app/models.rb` is being required and the
//! tree does not boot.
//!
//! WHAT IT DOES NOT REWRITE: a receiver the analyzer typed as a
//! Relation. That one has a real method with a real RBS signature, and
//! routing it through the module function would trade a typed call for
//! an untyped one to fix nothing.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_enumerable_ext_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
    // Test bodies too — untyped at this point (they are typed in
    // `test_module_to_library`), so only the untyped-gated methods
    // ground there; `many?`/`to_sentence` want a typed Array receiver
    // and stay as written, as they always did in a test.
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

/// The same grounding over one TYPED body, for
/// `test_module_to_library` to run once a test's body has types — a
/// `<<~HTML.squish` in a test is a String only after that pass, and
/// this module's own walk runs before it. Sibling of
/// `array_ordinal::rewrite_body`, called from the same place.
/// Returns whether any site was rewritten so the caller can skip a
/// follow-up type.
pub(crate) fn rewrite_body(expr: &mut Expr) -> bool {
    let mut changed = false;
    expr.node.for_each_child_mut(&mut |c| {
        if rewrite_body(c) {
            changed = true;
        }
    });
    rewrite_node(expr) || changed
}

fn rewrite(expr: &mut Expr) {
    let _ = rewrite_body(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) -> bool {
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node else {
        return false;
    };
    // `first(n)` / `last(n)` on the same Relation-or-Array union `many?`
    // meets below: campfire's direct-room sidebar goes on to
    // `members.first(4)` for a room of three or more. A proven Relation
    // is renamed to `first_n` by the scope-chain pass, a plain Array
    // already means it; the union is neither, and the Relation half's
    // `first` takes no count (ArgumentError, every sidebar render for a
    // member of a group direct room). Both halves answer `to_a`, and
    // on the Array that follows `first(n)` means what Ruby means.
    if matches!(method.as_str(), "first" | "last") && args.len() == 1 && block.is_none() {
        if let Some(array_ty) = recv.as_ref().and_then(|r| relation_or_array_half(r.ty.as_ref())) {
            let receiver = recv.take().expect("checked above");
            let mut to_a = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(receiver),
                    method: Symbol::from("to_a"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            );
            to_a.ty = Some(array_ty);
            *recv = Some(to_a);
            return true;
        }
        return false;
    }
    // `many?` accepts either form: its block counts matching elements,
    // while its bare form counts all elements. Keep the block attached
    // when grounding it to the shared runtime helper.
    if method.as_str() == "wrap" {
        return ground_array_wrap(expr);
    }
    // Rails' Array#excluding and its #without alias share this helper.
    // Keep Relation calls on ActiveRecord::Relation's typed methods, and
    // leave unions/untyped receivers alone: only a proven Array fits the
    // helper's generic signature without erasing its element type.
    if matches!(method.as_str(), "excluding" | "without") && block.is_none() {
        let Some(receiver) = recv.as_ref() else { return false };
        if !matches!(receiver.ty.as_ref(), Some(Ty::Array { .. })) {
            return false;
        }
        let receiver = recv.take().expect("checked above");
        *recv = Some(Expr::new(
            span,
            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
        ));
        args.insert(0, receiver);
        *method = Symbol::from("excluding");
        *parenthesized = true;
        return true;
    }
    let wants_block = match method.as_str() {
        "index_by" => true,
        "many?" => block.is_some(),
        "to_sentence" | "sole" | "squish" | "squish!" => false,
        _ => return false,
    };
    // `to_sentence` takes Rails' three connector options; the module
    // function takes them positionally, so a keyword Hash of those keys
    // (and no others) becomes the three arguments, defaults filled in.
    // The module function takes all three, so a bare call passes the
    // defaults: one signature, every target.
    let connectors = match (method.as_str(), args.len()) {
        ("to_sentence", 0) => Some(SENTENCE_DEFAULTS.map(String::from)),
        ("to_sentence", 1) => {
            let Some(c) = sentence_connectors(&args[0]) else { return false };
            Some(c)
        }
        _ => None,
    };
    if (connectors.is_none() && !args.is_empty()) || block.is_some() != wants_block {
        return false;
    }
    let Some(receiver) = recv.as_ref() else { return false };
    // A Relation-or-Array receiver — campfire's direct-room sidebar,
    // `members = room.users.without(user).presence || [user]`, then
    // `members.many?`. Neither grounding fits: the Relation half has a
    // real `many?` but the Array half has none on spinel (a poly
    // dispatch that 500'd every sidebar on the deployed binary), and the
    // module function takes only an Array. Both halves answer `size`,
    // which is what `many?` without a block asks, so the call becomes
    // `size > 1`. On the Relation half that loads the rows rather than
    // counting them — the rows the partial iterates next anyway.
    if method.as_str() == "many?"
        && block.is_none()
        && is_relation_or_array_union(receiver.ty.as_ref())
    {
        let receiver = recv.take().expect("checked above");
        let mut size = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(receiver),
                method: Symbol::from("size"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        size.ty = Some(Ty::Int);
        let mut one = Expr::new(
            span,
            ExprNode::Lit { value: crate::expr::Literal::Int { value: 1 } },
        );
        one.ty = Some(Ty::Int);
        *expr.node = ExprNode::Send {
            recv: Some(size),
            method: Symbol::from(">"),
            args: vec![one],
            block: None,
            parenthesized: false,
        };
        expr.ty = Some(Ty::Bool);
        return true;
    }
    if is_relation(receiver.ty.as_ref()) {
        return false;
    }
    // `many?` names an `Array` parameter, so only an Array receiver
    // goes. `index_by` keeps the wider gate it has always had (its
    // parameter is untyped, and an untyped receiver is the case the
    // header explains). A Hash or String receiver here stays visible
    // rather than becoming a call whose argument does not fit.
    if matches!(method.as_str(), "many?" | "to_sentence")
        && !matches!(receiver.ty.as_ref(), Some(Ty::Array { .. }))
        && !(method.as_str() == "to_sentence" && is_block_map(receiver))
    {
        return false;
    }
    // `squish` names a `String` parameter, so only a String receiver
    // goes — and it is the only one of these whose name a model could
    // plausibly define itself, which is the second reason to gate on
    // the analyzer's answer rather than on the spelling.
    if matches!(method.as_str(), "squish" | "squish!")
        && !matches!(receiver.ty.as_ref(), Some(Ty::Str))
    {
        return false;
    }
    let receiver = recv.take().expect("checked above");
    *recv = Some(Expr::new(
        span,
        ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
    ));
    args.clear();
    args.push(receiver);
    if let Some(connectors) = connectors {
        args.extend(connectors.into_iter().map(|value| {
            let mut lit = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Str { value } });
            lit.ty = Some(Ty::Str);
            lit
        }));
    }
    *parenthesized = true;
    if method.as_str() == "squish!" {
        expr.ty = Some(Ty::Str);
    }
    true
}

/// `Array.wrap(value)` → `ActiveSupport.wrap(value)`. A receiverless
/// `wrap`, or a wrap on something other than the Array class, stays.
fn ground_array_wrap(expr: &mut Expr) -> bool {
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized: _ } = &mut *expr.node else {
        return false;
    };
    if method.as_str() != "wrap" || block.is_some() || args.len() != 1 {
        return false;
    }
    let Some(receiver) = recv.as_ref() else { return false };
    let ExprNode::Const { path } = &*receiver.node else { return false };
    // `Reports::Array.wrap` is not ActiveSupport's method. Only the
    // top-level constant, written `Array` or `::Array`, is.
    let names: Vec<&str> = path.iter().map(|name| name.as_str()).collect();
    if names != ["Array"] && names != ["::Array"] {
        return false;
    }
    // Folded here, not hosted as `ActiveSupport.wrap`. That method
    // reads an untyped parameter, and each read counts against the
    // runtime concrete-type ceiling. Nil is `[]`. An Array is itself,
    // which is what Rails' `to_ary` answers for a real Array. A single
    // other closed type is a one-element array. A union of those
    // shapes is a branch, so neither arm wraps the other. Anything
    // else stays the call. A custom `to_ary` is not called: an unknown
    // `to_ary` is dropped, which would wrap the object instead of its
    // records.
    let arg = args[0].clone();
    let Some(folded) = fold_array_wrap(span, &arg) else { return false };
    expr.ty = folded.ty.clone();
    *expr.node = *folded.node;
    true
}

fn empty_array(span: crate::span::Span) -> Expr {
    let mut empty = Expr::new(span, ExprNode::Array { elements: vec![], style: Default::default() });
    // A later pass reads assignment types. An untyped `[]` is invisible
    // to it, so a controller ivar assigned both this and a Relation
    // would keep the Relation.
    empty.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
    empty
}

fn fold_array_wrap(span: crate::span::Span, arg: &Expr) -> Option<Expr> {
    let empty = empty_array(span);
    let one = |value: Expr| {
        let mut wrapped = Expr::new(
            span,
            ExprNode::Array { elements: vec![value], style: Default::default() },
        );
        wrapped.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
        wrapped
    };
    match arg.ty.as_ref() {
        Some(Ty::Nil) => Some(empty),
        Some(Ty::Array { .. }) => Some(arg.clone()),
        Some(Ty::Union { variants }) => fold_union_wrap(span, arg, variants),
        None => None,
        Some(_) => Some(one(arg.clone())),
    }
}

/// `Array | Nil` is `[]` or the array. `String | Nil` is `[]` or
/// `[value]`. A union that also holds an open or nested type is not
/// one of those answers, so the call stays.
fn fold_union_wrap(span: crate::span::Span, arg: &Expr, variants: &[Ty]) -> Option<Expr> {
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    let arrays: Vec<&Ty> = variants.iter().filter(|v| matches!(v, Ty::Array { .. })).collect();
    let others: Vec<&Ty> = variants
        .iter()
        .filter(|v| !matches!(v, Ty::Nil | Ty::Array { .. }))
        .collect();
    if arrays.len() > 1 || (!others.is_empty() && !arrays.is_empty()) {
        return None;
    }
    if !has_nil && arrays.is_empty() {
        return Some(Expr::new(
            span,
            ExprNode::Array { elements: vec![arg.clone()], style: Default::default() },
        ));
    }
    let bound = bind_once(span, arg);
    let read = bound.read.clone();
    let when_nil = empty_array(span);
    let when_present = if arrays.len() == 1 {
        read.clone()
    } else {
        let mut wrapped = Expr::new(
            span,
            ExprNode::Array { elements: vec![read.clone()], style: Default::default() },
        );
        wrapped.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
        wrapped
    };
    let mut cond = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(read),
            method: crate::ident::Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    cond.ty = Some(Ty::Bool);
    let mut branch = Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch: when_nil,
            else_branch: when_present,
        },
    );
    branch.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
    Some(Expr::new(
        span,
        ExprNode::Seq { exprs: vec![bound.assign, branch] },
    ))
}

struct Bound {
    assign: Expr,
    read: Expr,
}

fn bind_once(span: crate::span::Span, arg: &Expr) -> Bound {
    if matches!(&*arg.node, ExprNode::Var { .. } | ExprNode::Lit { .. }) {
        return Bound { assign: Expr::new(span, ExprNode::Seq { exprs: vec![] }), read: arg.clone() };
    }
    let name = crate::ident::Symbol::from("__array_wrap");
    let read = Expr::new(
        span,
        ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() },
    );
    let assign = Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
            value: arg.clone(),
        },
    );
    Bound { assign, read }
}

/// words_connector, two_words_connector, last_word_connector — Rails'
/// `:en` defaults.
const SENTENCE_DEFAULTS: [&str; 3] = [", ", " and ", ", and "];

/// Rails' `to_sentence` connectors (`:en` defaults), in the module
/// function's positional order, from a keyword Hash of string literals.
/// Anything else — a locale, a computed value, an unknown key — answers
/// None and the call is left as written, visible, not guessed at.
fn sentence_connectors(arg: &Expr) -> Option<[String; 3]> {
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let mut out = SENTENCE_DEFAULTS.map(String::from);
    for (k, v) in entries {
        let ExprNode::Lit { value: crate::expr::Literal::Sym { value: key } } = &*k.node else { return None };
        let ExprNode::Lit { value: crate::expr::Literal::Str { value } } = &*v.node else { return None };
        let slot = match key.as_str() {
            "words_connector" => 0,
            "two_words_connector" => 1,
            "last_word_connector" => 2,
            _ => return None,
        };
        out[slot] = value.clone();
    }
    Some(out)
}

/// A union of an Array with a Relation or an untyped half — campfire's
/// `members` types `Array[User?] | untyped`, the `.presence` of a
/// `without` chain being the untyped side. Every variant answers `size`;
/// nothing else (a Hash, nil) is let through.
fn is_relation_or_array_union(ty: Option<&Ty>) -> bool {
    let Some(Ty::Union { variants }) = ty else { return false };
    variants.iter().any(|v| matches!(v, Ty::Array { .. }))
        && variants.iter().all(|v| matches!(v, Ty::Array { .. } | Ty::Relation { .. } | Ty::Untyped))
}

/// A block `map`/`collect`/`filter_map`/`flat_map` — an Array whatever
/// the receiver was, since that is Enumerable's contract and a Relation
/// maps through its loaded rows. campfire's group-room initials are
/// `members.map { … }.to_sentence(…)` over the Relation-or-Array union,
/// which the analyzer leaves untyped; ungrounded, spinel had no
/// `to_sentence` on the Array it got (NoMethodError, every sidebar render
/// for a member of a group direct room).
fn is_block_map(receiver: &Expr) -> bool {
    matches!(
        &*receiver.node,
        ExprNode::Send { method, block: Some(_), .. }
            if matches!(method.as_str(), "map" | "collect" | "filter_map" | "flat_map")
    )
}

/// The Array variant of such a union — the type its `to_a` answers.
fn relation_or_array_half(ty: Option<&Ty>) -> Option<Ty> {
    if !is_relation_or_array_union(ty) {
        return None;
    }
    let Some(Ty::Union { variants }) = ty else { return None };
    variants.iter().find(|v| matches!(v, Ty::Array { .. })).cloned()
}

/// `Ty::Relation` under any element type, and through a nullable union
/// — the shape a scope chain leaves behind.
fn is_relation(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Relation { .. }) => true,
        Some(Ty::Union { variants }) => variants.iter().any(|v| is_relation(Some(v))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn many_on(ty: Ty) -> Expr {
        let mut members = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("members") },
        );
        members.ty = Some(ty);
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(members),
                method: Symbol::from("many?"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }

    fn array_of_users() -> Ty {
        Ty::Array { elem: Box::new(Ty::Class { id: crate::ident::ClassId(Symbol::from("User")), args: vec![] }) }
    }

    fn method_of(e: &Expr) -> String {
        match &*e.node {
            ExprNode::Send { method, .. } => method.as_str().to_string(),
            other => format!("{other:?}"),
        }
    }

    fn array_exclusion(method: &str, receiver_ty: Ty, excluded: usize) -> Expr {
        let mut receiver = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("items") },
        );
        receiver.ty = Some(receiver_ty);
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(receiver),
                method: Symbol::from(method),
                args: (0..excluded)
                    .map(|value| Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Int { value: value as i64 } }))
                    .collect(),
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn array_excluding_and_without_ground_to_the_shared_helper() {
        for name in ["excluding", "without"] {
            let mut e = array_exclusion(name, array_of_users(), 2);
            assert!(rewrite_node(&mut e));
            assert_eq!(method_of(&e), "excluding");
            let ExprNode::Send { recv: Some(helper), args, parenthesized, .. } = &*e.node else { panic!() };
            assert!(*parenthesized);
            assert!(matches!(&*helper.node, ExprNode::Const { path } if path[0].as_str() == "ActiveSupport"));
            assert_eq!(args.len(), 3, "receiver plus both exclusions");
        }
        let mut no_args = array_exclusion("excluding", array_of_users(), 0);
        assert!(rewrite_node(&mut no_args));
        let ExprNode::Send { args, .. } = &*no_args.node else { panic!() };
        assert_eq!(args.len(), 1, "the empty Rails exclusion list still reaches the helper");
    }

    #[test]
    fn relation_excluding_keeps_its_typed_relation_method() {
        let model = crate::ident::ClassId(Symbol::from("User"));
        let mut e = array_exclusion("excluding", Ty::Relation { of: model }, 1);
        assert!(!rewrite_node(&mut e));
        assert_eq!(method_of(&e), "excluding");
        let ExprNode::Send { recv: Some(receiver), .. } = &*e.node else { panic!() };
        assert!(matches!(&*receiver.node, ExprNode::Var { .. }));
    }

    /// campfire's direct-room sidebar: `presence || [user]` types
    /// `Array | untyped`, and the Array half has no `many?` on spinel.
    #[test]
    fn many_on_an_array_or_untyped_union_is_a_size_test() {
        let mut e = many_on(Ty::Union { variants: vec![array_of_users(), Ty::Untyped] });
        rewrite(&mut e);
        assert_eq!(method_of(&e), ">");
        let ExprNode::Send { recv: Some(size), .. } = &*e.node else { panic!() };
        assert_eq!(method_of(size), "size");
        assert_eq!(e.ty, Some(Ty::Bool));
    }

    /// A plain Array still takes the module function.
    #[test]
    fn many_on_an_array_grounds_to_the_module_function() {
        let mut e = many_on(array_of_users());
        rewrite(&mut e);
        assert_eq!(method_of(&e), "many?");
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { panic!() };
        assert!(matches!(&*r.node, ExprNode::Const { path } if path[0].as_str() == "ActiveSupport"));
    }

    #[test]
    fn many_with_a_block_grounds_without_dropping_the_block() {
        let mut e = many_on(array_of_users());
        let ExprNode::Send { block, .. } = &mut *e.node else {
            panic!()
        };
        *block = Some(Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: crate::expr::Literal::Bool { value: true },
            },
        ));
        rewrite(&mut e);
        let ExprNode::Send {
            recv: Some(r),
            block: Some(_),
            ..
        } = &*e.node
        else {
            panic!()
        };
        assert!(
            matches!(&*r.node, ExprNode::Const { path } if path[0].as_str() == "ActiveSupport")
        );
    }

    #[test]
    fn squish_bang_grounds_and_uses_its_non_nil_rails_return_type() {
        let mut e = many_on(Ty::Str);
        let ExprNode::Send { method, .. } = &mut *e.node else {
            panic!()
        };
        *method = Symbol::from("squish!");
        rewrite(&mut e);
        let ExprNode::Send {
            recv: Some(r),
            args,
            ..
        } = &*e.node
        else {
            panic!()
        };
        assert_eq!(method_of(&e), "squish!");
        assert!(
            matches!(&*r.node, ExprNode::Const { path } if path[0].as_str() == "ActiveSupport")
        );
        assert_eq!(args.len(), 1);
        assert_eq!(e.ty, Some(Ty::Str));
    }

    /// A union with a variant that has no `size` (nil) is left alone.
    #[test]
    fn many_on_a_nilable_array_union_is_untouched() {
        let mut e = many_on(Ty::Union { variants: vec![array_of_users(), Ty::Nil] });
        rewrite(&mut e);
        assert_eq!(method_of(&e), "many?");
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { panic!() };
        assert!(matches!(&*r.node, ExprNode::Var { .. }));
    }

    fn first_n_on(ty: Ty) -> Expr {
        let mut e = many_on(ty);
        let ExprNode::Send { method, args, .. } = &mut *e.node else { panic!() };
        *method = Symbol::from("first");
        args.push(Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Int { value: 4 } }));
        e
    }

    /// campfire's group direct room: `members.first(4)` on the same
    /// `Array | untyped` union reads through `to_a`, typed as the Array half.
    #[test]
    fn first_n_on_an_array_or_untyped_union_reads_through_to_a() {
        let mut e = first_n_on(Ty::Union { variants: vec![array_of_users(), Ty::Untyped] });
        rewrite(&mut e);
        assert_eq!(method_of(&e), "first");
        let ExprNode::Send { recv: Some(to_a), args, .. } = &*e.node else { panic!() };
        assert_eq!(args.len(), 1);
        assert_eq!(method_of(to_a), "to_a");
        assert_eq!(to_a.ty, Some(array_of_users()));
    }

    /// A plain Array's `first(n)` already means what Ruby means.
    #[test]
    fn first_n_on_an_array_is_untouched() {
        let mut e = first_n_on(array_of_users());
        rewrite(&mut e);
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { panic!() };
        assert!(matches!(&*r.node, ExprNode::Var { .. }));
    }

    fn sentence_on(options: Vec<(&str, &str)>) -> Expr {
        let mut e = many_on(Ty::Array { elem: Box::new(Ty::Str) });
        let ExprNode::Send { method, args, .. } = &mut *e.node else { panic!() };
        *method = Symbol::from("to_sentence");
        if !options.is_empty() {
            let lit = |v| Expr::new(Span::synthetic(), ExprNode::Lit { value: v });
            let entries = options
                .into_iter()
                .map(|(k, v)| {
                    (
                        lit(crate::expr::Literal::Sym { value: Symbol::from(k) }),
                        lit(crate::expr::Literal::Str { value: v.to_string() }),
                    )
                })
                .collect();
            args.push(Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: true }));
        }
        e
    }

    fn string_args(e: &Expr) -> Vec<String> {
        let ExprNode::Send { args, .. } = &*e.node else { panic!() };
        args.iter()
            .filter_map(|a| match &*a.node {
                ExprNode::Lit { value: crate::expr::Literal::Str { value } } => Some(value.clone()),
                _ => None,
            })
            .collect()
    }

    /// A bare `to_sentence` passes the :en connectors to the module function.
    #[test]
    fn to_sentence_passes_the_default_connectors() {
        let mut e = sentence_on(vec![]);
        rewrite(&mut e);
        assert_eq!(string_args(&e), [", ", " and ", ", and "]);
    }

    /// campfire's group-room initials: `to_sentence(two_words_connector: '+')`.
    #[test]
    fn to_sentence_maps_a_named_connector_into_its_slot() {
        let mut e = sentence_on(vec![("two_words_connector", "+")]);
        rewrite(&mut e);
        assert_eq!(string_args(&e), [", ", "+", ", and "]);
    }

    /// An option the module function has no slot for (a locale) is left as written.
    #[test]
    fn to_sentence_with_an_unknown_option_is_untouched() {
        let mut e = sentence_on(vec![("locale", "fr")]);
        rewrite(&mut e);
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { panic!() };
        assert!(matches!(&*r.node, ExprNode::Var { .. }));
    }

    /// `members.map { … }.to_sentence(…)` over an untyped receiver still
    /// grounds: a block `map` answers an Array whatever it was called on.
    #[test]
    fn to_sentence_on_an_untyped_block_map_grounds() {
        let mut e = sentence_on(vec![("two_words_connector", "+")]);
        let ExprNode::Send { recv: Some(r), .. } = &mut *e.node else { panic!() };
        let mut members = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("members") },
        );
        members.ty = Some(Ty::Untyped);
        let block = Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil });
        *r = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(members),
                method: Symbol::from("map"),
                args: Vec::new(),
                block: Some(block),
                parenthesized: false,
            },
        );
        r.ty = Some(Ty::Untyped);
        rewrite(&mut e);
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { panic!() };
        assert!(matches!(&*r.node, ExprNode::Const { path } if path[0].as_str() == "ActiveSupport"));
        assert_eq!(string_args(&e), [", ", "+", ", and "]);
    }
}
