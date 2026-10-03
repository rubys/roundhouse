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

pub(super) fn reject_entrypoint(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
    context: &str,
) -> IngestResult<()> {
    if let Some(formal) = parse(def).anonymous {
        let kind = match formal {
            AnonymousFormal::Forwarding => "full forwarding",
            AnonymousFormal::KeywordRest => "anonymous keyword forwarding",
        };
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("{kind} declaration on a {context} is not preserved yet"),
        });
    }
    Ok(())
}
