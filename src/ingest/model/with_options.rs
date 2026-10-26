//! `with_options` in a model class body (or a concern's `included do`):
//!
//! ```ruby
//! with_options on: :create, if: :local? do
//!   validates :followers_url, absence: true
//! end
//!
//! with_options class_name: 'Account', optional: true do
//!   belongs_to :approved_by_account
//! end
//! ```
//!
//! Rails' `OptionMerger` (activerecord 8.1.4) merges the options into each
//! call in the block, the call's own keys winning: an inner `if:` replaces
//! an outer `if:`, an inner `on:` an outer `on:`, an inner `class_name:`
//! the outer one, while an inner `unless:` sits beside an outer `if:`.
//! Nested blocks merge outward in, and `with_options(…) do |o| o.validates
//! … end` merges into the calls made on `o`. Each call is replayed with the
//! merged options written into its source (`ingest::with_options`) and
//! ingested as if it had been spelled that way.
//!
//! All or nothing per block: anything the replay cannot represent (a
//! non-literal option hash such as a local or a constant, a call with a
//! block of its own, a statement that is not a call, a merged `if:` that
//! is neither a Symbol nor a lambda) declines the whole block, reported
//! with what blocked it. A condition silently lost from one validation
//! because a sibling statement was unsupported would be worse than the gap.

use indexmap::IndexMap;
use ruby_prism::{CallNode, Node};

use crate::dialect::{Comment, ModelBodyItem};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::ingest::with_options::{self as merge, MergedOption};
use crate::ingest::{IngestResult, prism_api::PrismLocation};

use super::super::util::{constant_id_str, flatten_statements};
use super::ClassConsts;

/// Whether `call` is a receiverless `with_options`.
pub(super) fn is_with_options(call: &CallNode<'_>) -> bool {
    call.receiver().is_none() && constant_id_str(&call.name()) == "with_options"
}

/// The body items the block expands to, or the decline that names what
/// blocked it.
pub(super) fn expand(
    call: &CallNode<'_>,
    outer: &[MergedOption],
    owner: &ClassId,
    file: &str,
    class_consts: &ClassConsts,
) -> IngestResult<Vec<ModelBodyItem>> {
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else {
        return Err(merge::gap(file, "only the block form is composed"));
    };
    let param = match block.parameters() {
        None => None,
        Some(params) => Some(block_param(&params).ok_or_else(|| {
            merge::gap(file, "a block with other than one plain parameter (`|o|`) is not composed")
        })?),
    };
    let merged = merge::block_options(call, outer, file)?;
    let Some(body) = block.body() else { return Ok(Vec::new()) };
    let mut items = Vec::new();
    for stmt in flatten_statements(body) {
        let Some(inner) = stmt.as_call_node() else {
            return Err(merge::gap(file, "only calls inside the block are composed"));
        };
        let name = constant_id_str(&inner.name()).to_string();
        // With a block parameter, Rails merges only into the calls made
        // on it; a receiverless call there is the class's own, as written.
        let (options, receiver_len): (&[MergedOption], usize) = match (inner.receiver(), &param) {
            (None, None) => (&merged, 0),
            (None, Some(_)) => (&[], 0),
            (Some(recv), Some(param))
                if recv.as_local_variable_read_node().is_some_and(|v| constant_id_str(&v.name()) == param.as_str()) =>
            {
                let Some(message) = inner.message_loc() else {
                    return Err(merge::gap(file, "call has no name to merge into"));
                };
                (&merged, message.start_offset() - inner.location().start_offset())
            }
            _ => return Err(merge::gap(file, &format!("`{name}` on another receiver is not composed"))),
        };
        if name == "with_options" {
            if receiver_len != 0 {
                return Err(merge::gap(file, "a nested `with_options` on the block parameter is not composed"));
            }
            items.extend(expand(&inner, options, owner, file, class_consts)?);
            continue;
        }
        if inner.block().is_some() {
            return Err(merge::gap(
                file,
                &format!("`{name}` with a block is not composed (Rails merges the options into its nested calls too)"),
            ));
        }
        let comments: Vec<Comment> = Vec::new();
        let no_defaults: Option<&mut IndexMap<Symbol, Expr>> = None;
        match merge::merged_call_source(&inner, options, receiver_len, true, file)? {
            None => items.extend(super::ingest_model_body_items(&stmt, owner, file, comments, no_defaults, class_consts)?),
            Some(source) => {
                let result = super::super::prism::parse(&source, file);
                let root = result.node();
                let Some(reparsed) = root.as_program_node().and_then(|p| p.statements().body().iter().next()) else {
                    return Err(merge::gap(file, "merged call did not re-parse"));
                };
                items.extend(super::ingest_model_body_items(&reparsed, owner, file, comments, no_defaults, class_consts)?);
            }
        }
    }
    Ok(items)
}

/// The one plain required parameter of `|o|`, or `None` for anything else.
fn block_param(params: &Node<'_>) -> Option<String> {
    let block_params = params.as_block_parameters_node()?;
    if block_params.locals().iter().next().is_some() {
        return None;
    }
    let p = block_params.parameters()?;
    let requireds: Vec<Node<'_>> = p.requireds().iter().collect();
    let [only] = requireds.as_slice() else { return None };
    if p.optionals().iter().next().is_some()
        || p.rest().is_some()
        || p.posts().iter().next().is_some()
        || p.keywords().iter().next().is_some()
        || p.keyword_rest().is_some()
        || p.block().is_some()
    {
        return None;
    }
    Some(constant_id_str(&only.as_required_parameter_node()?.name()).to_string())
}
