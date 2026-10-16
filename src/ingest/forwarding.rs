//! Shared anonymous declaration forms. The Prism keyword-rest-slot
//! recognition comes from Tim Tischler's F7 commit 013588ec (pr/argument-forwarding).
//! Keep the anonymous contract intact instead of synthesizing capturable locals.
//!
//! ONE NARROW EXCEPTION, for the anonymous positional rest `def f(*)`: when
//! the body never forwards it (no bare `*` splat anywhere in the def), the
//! rest is unreachable from the body, so binding it to a generated name
//! that the source does not use (checked like the multi-write temps)
//! changes nothing a program can observe but its arity, which it keeps.
//! campfire's `ApplicationHelper#token_tag(*)` is that shape. A body that
//! forwards (`foo(*)`) still reports `AnonymousRest`: the ingest would turn
//! that bare splat into `*nil`, and forwarding is not modeled yet.

use super::{IngestError, IngestResult};
use crate::dialect::UnsupportedFormal;

pub(crate) use crate::dialect::AnonymousFormal;

#[derive(Default)]
pub(crate) struct Formals {
    pub anonymous: Option<AnonymousFormal>,
    pub unsupported: Option<UnsupportedFormal>,
    pub has_anonymous_block: bool,
    /// The generated name an unforwarded anonymous `*` binds to (see the
    /// module doc); `None` when there is no such rest.
    pub anonymous_rest_name: Option<crate::ident::Symbol>,
}

/// Is there a bare `*` (a splat with no operand) anywhere under `node`?
fn forwards_anonymous_rest(node: &ruby_prism::Node<'_>) -> bool {
    struct V {
        found: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for V {
        // A nested `def` binds its own parameters: its bare `*` forwards
        // its own rest, not this one. Blocks stay walked, since a bare
        // `*` inside one forwards the enclosing method's rest.
        fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {}

        fn visit_splat_node(&mut self, node: &ruby_prism::SplatNode<'pr>) {
            if node.expression().is_none() {
                self.found = true;
                return;
            }
            ruby_prism::visit_splat_node(self, node);
        }
    }
    let mut v = V { found: false };
    ruby_prism::Visit::visit(&mut v, node);
    v.found
}

/// A name for the unforwarded anonymous rest that nothing in the def's
/// own source spells, so it cannot capture or shadow a user local.
fn anonymous_rest_name(def: &ruby_prism::DefNode<'_>) -> crate::ident::Symbol {
    let location = def.location();
    let stem = format!("__anon_rest_{}", location.start_offset());
    let mut name = stem.clone();
    let mut suffix = 0;
    while super::sources::generated_local_is_reserved(&location, &name) {
        suffix += 1;
        name = format!("{stem}_{suffix}");
    }
    crate::ident::Symbol::from(name)
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
    let anonymous_rest = pn.rest().is_some_and(|p| {
        p.as_rest_parameter_node()
            .is_some_and(|p| p.name().is_none())
    });
    let forwarded = anonymous_rest && def.body().is_some_and(|b| forwards_anonymous_rest(&b));
    let unsupported = if pn
        .requireds()
        .iter()
        .chain(pn.posts().iter())
        .any(|p| p.as_required_parameter_node().is_none())
    {
        Some(UnsupportedFormal::Destructured)
    } else if forwarded {
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
        anonymous_rest_name: (anonymous_rest && !forwarded).then(|| anonymous_rest_name(def)),
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
