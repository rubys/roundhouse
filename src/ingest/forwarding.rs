//! Shared anonymous declaration forms. The Prism keyword-rest-slot
//! recognition comes from Tim Tischler's F7 commit 013588ec (pr/argument-forwarding).
//! Keep the anonymous contract intact instead of synthesizing capturable locals.

use super::{IngestError, IngestResult};
use crate::dialect::{Param, UnsupportedFormal};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnonymousFormal {
    Forwarding,
    KeywordRest,
}

impl AnonymousFormal {
    pub(super) fn into_param(self) -> Param {
        match self {
            Self::Forwarding => Param::forwarding(),
            Self::KeywordRest => {
                // Empty is a nameless declaration, never a legal binding.
                let mut param = Param::keyword("".into(), None);
                param.rest = true;
                param
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Formals {
    pub anonymous: Option<AnonymousFormal>,
    pub unsupported: Option<UnsupportedFormal>,
    pub has_anonymous_block: bool,
}

/// Parse source facts once, before the library/model parameter projections.
/// Unsupported formals stay on MethodDef, not on a rewritable body expression.
pub(crate) fn parse(def: &ruby_prism::DefNode<'_>) -> Formals {
    let Some(pn) = def.parameters() else {
        return Formals::default();
    };
    let anonymous = pn.keyword_rest().and_then(|node| {
        if node.as_forwarding_parameter_node().is_some() {
            Some(AnonymousFormal::Forwarding)
        } else if node
            .as_keyword_rest_parameter_node()
            .is_some_and(|p| p.name().is_none())
        {
            Some(AnonymousFormal::KeywordRest)
        } else {
            None
        }
    });
    let unsupported = if pn
        .requireds()
        .iter()
        .chain(pn.posts().iter())
        .any(|p| p.as_required_parameter_node().is_none())
    {
        Some(UnsupportedFormal::Destructured)
    } else if pn.rest().is_some_and(|p| {
        p.as_rest_parameter_node()
            .is_some_and(|p| p.name().is_none())
    }) {
        Some(UnsupportedFormal::AnonymousRest)
    } else if pn
        .keyword_rest()
        .is_some_and(|p| p.as_no_keywords_parameter_node().is_some())
    {
        Some(UnsupportedFormal::NoKeywords)
    } else {
        None
    };
    Formals {
        anonymous,
        unsupported,
        has_anonymous_block: pn.block().is_some_and(|b| b.name().is_none()),
    }
}

/// A bare `**` call forwards the enclosing method's nameless keyword
/// rest. Nested method bodies are their own declarations, so an outer
/// `**` does not license an inner one. Anything else stays an error
/// instead of becoming an empty hash.
pub(super) fn require_anonymous_keyword_declaration(
    anonymous: Option<AnonymousFormal>,
    body: &crate::expr::Expr,
    file: &str,
) -> IngestResult<()> {
    if anonymous == Some(AnonymousFormal::KeywordRest) || !body_forwards_anonymous_keywords(body) {
        return Ok(());
    }
    Err(IngestError::Unsupported {
        file: file.into(),
        message: "anonymous `**` keyword forwarding requires an anonymous keyword-rest declaration".into(),
    })
}

fn body_forwards_anonymous_keywords(body: &crate::expr::Expr) -> bool {
    fn is_forward(expr: &crate::expr::Expr) -> bool {
        matches!(&*expr.node, crate::expr::ExprNode::KeywordSplat { value }
            if matches!(&*value.node, crate::expr::ExprNode::Var { name, .. } if name.as_str().is_empty()))
    }
    if is_forward(body) {
        return true;
    }
    let mut found = false;
    body.node.for_each_child(&mut |child| {
        if found || matches!(&*child.node, crate::expr::ExprNode::Lambda { .. }) {
            return;
        }
        found = body_forwards_anonymous_keywords(child);
    });
    found
}

pub(super) fn reject_entrypoint(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
    context: &str,
) -> IngestResult<()> {
    if parse(def).anonymous == Some(AnonymousFormal::Forwarding) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("full forwarding declaration on a {context} is not preserved yet"),
        });
    }
    Ok(())
}
