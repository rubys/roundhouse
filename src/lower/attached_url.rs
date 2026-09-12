//! An attachment in a URL position becomes its URL:
//! `image_tag(user.avatar)` → `image_tag(user.avatar.url)`,
//! `url_for(attachment.preview(…))` → `url_for(attachment.preview(…).url)`.
//!
//! Rails resolves these through `polymorphic_url`: `image_tag` calls
//! `url_for` on anything that is not a String, and an
//! `ActiveStorage::Attached` / `VariantWithRecord` answers its engine
//! route. Here `ActionView::ViewHelpers.image_tag` / `image_path` /
//! `url_for` take a String — the one shape every target compiles — so
//! the resolution moves to the call site: the attachment-shaped
//! argument asks itself (`Attached#url`, `VariantWithRecord#url`, both
//! in `runtime/ruby/active_storage.rb`) and the helper gets the String
//! it is typed for. A branch of an `if` in that position is rewritten
//! on its own, so campfire's `image_tag @user.avatar.attached? ?
//! @user.avatar : "default-avatar.svg"` hands `image_tag` a String
//! either way.
//!
//! BY NAME, not by type. View bodies are typed per method against a
//! framework-stub registry (`view_to_library::type_method_body`) that
//! knows no model, so `user.avatar` carries no type there — but the
//! app's `has_one_attached` declarations are known app-wide, and a
//! zero-arg send named for one of them, with a receiver, is that
//! reader (the same convention `view_to_library::ivar_ty` reads a
//! view local's type from). `variant` / `representation` / `preview` /
//! `processed` chained off one are the variant. A helper method
//! parameter that HOLDS an attachment (`broadcast_image_tag(image,
//! …)`) is not a send by that name and is left to the runtime's
//! `polymorphic_url` reopen, which narrows by class at run time.
//!
//! Positions: the first argument of `image_tag`, `image_path`,
//! `url_for`, `polymorphic_url` and `polymorphic_path`, with or
//! without the `ActionView::ViewHelpers.` receiver the view lowering
//! adds.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use std::collections::HashSet;

const URL_POSITION_HELPERS: &[&str] =
    &["image_tag", "image_path", "url_for", "polymorphic_url", "polymorphic_path"];

const VARIANT_METHODS: &[&str] = &["variant", "representation", "preview", "processed"];

pub fn apply_attached_url_lowering(app: &mut App) {
    let attrs: HashSet<Symbol> = app
        .models
        .iter()
        .flat_map(|m| super::attached::attached_attrs(m).into_iter().map(|(_s, a)| a))
        .collect();
    if attrs.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |body| rewrite(body, &attrs));
    for view in &mut app.views {
        rewrite(&mut view.body, &attrs);
    }
}

fn rewrite(e: &mut Expr, attrs: &HashSet<Symbol>) {
    e.node.for_each_child_mut(&mut |child| rewrite(child, attrs));
    let ExprNode::Send { method, args, .. } = &mut *e.node else { return };
    if !URL_POSITION_HELPERS.contains(&method.as_str()) || args.is_empty() {
        return;
    }
    urlize(&mut args[0], attrs);
}

/// Replace an attachment-shaped expression with `<expr>.url`, looking
/// through the branches of an `if` (the ternary a view writes to pick
/// between an attachment and a stock asset).
fn urlize(e: &mut Expr, attrs: &HashSet<Symbol>) {
    match &mut *e.node {
        ExprNode::If { then_branch, else_branch, .. } => {
            urlize(then_branch, attrs);
            urlize(else_branch, attrs);
        }
        ExprNode::Seq { exprs } => {
            if let Some(last) = exprs.last_mut() {
                urlize(last, attrs);
            }
        }
        _ => {
            if is_attachment_shaped(e, attrs) {
                let span = e.span;
                let inner = std::mem::replace(
                    e,
                    Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Nil }),
                );
                let mut url = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(inner),
                        method: Symbol::from("url"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                );
                url.ty = Some(crate::ty::Ty::Str);
                *e = url;
            }
        }
    }
}

/// `recv.<attached attr>` (the reader), or a variant chained off one.
fn is_attachment_shaped(e: &Expr, attrs: &HashSet<Symbol>) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if attrs.contains(method) && args.is_empty() {
        return true;
    }
    VARIANT_METHODS.contains(&method.as_str()) && is_attachment_shaped(recv, attrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Literal;
    use crate::span::Span;

    fn attrs() -> HashSet<Symbol> {
        [Symbol::from("avatar")].into_iter().collect()
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from(name) },
        )
    }

    fn str_lit(v: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: v.into() } })
    }

    fn first_arg_method(e: &Expr) -> String {
        let ExprNode::Send { args, .. } = &*e.node else { panic!("send") };
        let ExprNode::Send { method, .. } = &*args[0].node else { panic!("send arg") };
        method.as_str().to_string()
    }

    #[test]
    fn reader_in_image_tag_asks_for_its_url() {
        let mut e = send(None, "image_tag", vec![send(Some(var("user")), "avatar", vec![])]);
        rewrite(&mut e, &attrs());
        assert_eq!(first_arg_method(&e), "url");
    }

    #[test]
    fn variant_chain_in_url_for_asks_for_its_url() {
        let variant = send(
            Some(send(Some(var("message")), "avatar", vec![])),
            "representation",
            vec![str_lit("thumb")],
        );
        let mut e = send(None, "url_for", vec![variant]);
        rewrite(&mut e, &attrs());
        assert_eq!(first_arg_method(&e), "url");
    }

    /// The ternary campfire writes: only the attachment branch changes.
    #[test]
    fn if_branches_are_rewritten_separately() {
        let pick = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: var("c"),
                then_branch: send(Some(var("user")), "avatar", vec![]),
                else_branch: str_lit("default-avatar.svg"),
            },
        );
        let mut e = send(None, "image_tag", vec![pick]);
        rewrite(&mut e, &attrs());
        let ExprNode::Send { args, .. } = &*e.node else { panic!() };
        let ExprNode::If { then_branch, else_branch, .. } = &*args[0].node else { panic!() };
        assert!(matches!(&*then_branch.node, ExprNode::Send { method, .. } if method.as_str() == "url"));
        assert!(matches!(&*else_branch.node, ExprNode::Lit { .. }));
    }

    /// A String argument, or a send by some other name, is not touched:
    /// the rule is the declared attribute's name.
    #[test]
    fn other_arguments_are_left_alone() {
        let mut e = send(None, "image_tag", vec![send(Some(var("user")), "name", vec![])]);
        rewrite(&mut e, &attrs());
        assert_eq!(first_arg_method(&e), "name");
    }
}
