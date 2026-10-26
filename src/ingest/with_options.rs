//! ActiveSupport's `Object#with_options`, replayed at ingest: the merge a
//! controller body (`controller::ingest_controller_with_options`) and a
//! model body (`model::ingest_model_with_options`) share.
//!
//! Rails' `OptionMerger` deep-merges the block's options into the
//! trailing hash of every call it receives, the call's own keys winning
//! (activerecord 8.1.4: an inner `if:` replaces an outer `if:`, an inner
//! `unless:` sits beside it), or appends them as keywords when the call
//! has none. Each call is replayed by re-reading its source with the
//! added options written in, keeping its byte offsets, so diagnostics
//! still point into the original file.

use ruby_prism::Node;

use super::prism_api::PrismLocation;

use super::util::{constant_id_str, symbol_value};
use super::{IngestError, IngestResult};

/// One `key: value` option a `with_options` block hands to the calls
/// inside it: the symbol key, the source of the pair, whether the value
/// is a hash literal (the one case `deep_merge` would combine), and, for
/// an `if:`/`unless:` key only, whether the value is a Symbol or a
/// lambda/proc (the conditions the filter and validation readers carry).
#[derive(Clone)]
pub(super) struct MergedOption {
    pub(super) key: String,
    pub(super) source: String,
    pub(super) hash_value: bool,
    pub(super) cond_ok: bool,
}

pub(super) fn gap(file: &str, detail: &str) -> IngestError {
    IngestError::Unsupported {
        file: file.into(),
        message: format!("with_options: {detail}"),
    }
}

/// Whether `node` is a Symbol or a lambda/proc literal.
fn is_recognized_cond_value(node: &Node<'_>) -> bool {
    if node.as_symbol_node().is_some() || node.as_lambda_node().is_some() {
        return true;
    }
    node.as_call_node().is_some_and(|call| {
        matches!(constant_id_str(&call.name()), "proc" | "lambda") && call.block().is_some()
    })
}

/// The `key: value` pairs of a literal option hash, or `None` when the
/// hash splats (`**opts`): its keys are then unknown. A non-symbol key is
/// `None` under `symbol_keys_only`, and otherwise skipped (a call's own
/// string-keyed pair never collides with a merged symbol key).
pub(super) fn literal_option_pairs(node: &Node<'_>, symbol_keys_only: bool) -> Option<Vec<MergedOption>> {
    let elements = if let Some(hash) = node.as_keyword_hash_node() {
        hash.elements()
    } else {
        node.as_hash_node()?.elements()
    };
    let mut pairs = Vec::new();
    for element in elements.iter() {
        let assoc = element.as_assoc_node()?;
        let Some(key) = symbol_value(&assoc.key()) else {
            if symbol_keys_only {
                return None;
            }
            continue;
        };
        let location = assoc.location();
        let value = assoc.value();
        pairs.push(MergedOption {
            cond_ok: !matches!(key.as_str(), "if" | "unless") || is_recognized_cond_value(&value),
            key,
            source: String::from_utf8_lossy(location.as_slice()).into_owned(),
            hash_value: value.as_hash_node().is_some() || value.as_keyword_hash_node().is_some(),
        });
    }
    Some(pairs)
}

/// `inner` deep-merged over `outer`, as `Hash#deep_merge` orders it: an
/// inner key replaces the outer one in place, new keys follow. Two hash
/// values under one key would be merged key by key; that is declined
/// rather than guessed.
pub(super) fn deep_merge(outer: &[MergedOption], inner: Vec<MergedOption>, file: &str) -> IngestResult<Vec<MergedOption>> {
    let mut merged = outer.to_vec();
    for option in inner {
        match merged.iter_mut().find(|o| o.key == option.key) {
            Some(existing) if existing.hash_value && option.hash_value => {
                return Err(gap(file, &format!("deep-merging two hashes under `{}:` is not composed", option.key)));
            }
            Some(existing) => *existing = option,
            None => merged.push(option),
        }
    }
    Ok(merged)
}

/// The block's single literal option hash, merged over `outer`.
pub(super) fn block_options(call: &ruby_prism::CallNode<'_>, outer: &[MergedOption], file: &str) -> IngestResult<Vec<MergedOption>> {
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [options] = args.as_slice() else {
        return Err(gap(file, "expects exactly one literal option hash"));
    };
    let Some(own) = literal_option_pairs(options, true) else {
        return Err(gap(file, "options must be a literal hash with symbol keys and no splat (a local or a constant is not read)"));
    };
    deep_merge(outer, own, file)
}

/// A call inside the block, re-read with the merged `options` it does not
/// spell itself written into its trailing hash. `receiver_len` bytes from
/// the call's start (a block parameter and its dot, `o.`) are blanked, so
/// the call reads receiverless at its own offsets. `Ok(None)` when
/// nothing is added and nothing blanked: the call is used as written.
/// `checked_if` is the merged condition check for callers that carry only
/// a Symbol or a lambda/proc there.
pub(super) fn merged_call_source(
    call: &ruby_prism::CallNode<'_>,
    options: &[MergedOption],
    receiver_len: usize,
    checked_if: bool,
    file: &str,
) -> IngestResult<Option<Vec<u8>>> {
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    // A braced positional hash (`validates :a, { if: :b }`) is not read as
    // options, so merging into it would claim a condition that never
    // reaches the call.
    if args.last().is_some_and(|a| a.as_hash_node().is_some()) {
        return Err(gap(file, "a positional option hash is not composed"));
    }
    let trailing = args.last().filter(|a| a.as_keyword_hash_node().is_some());
    let own = match trailing {
        Some(hash) => match literal_option_pairs(hash, false) {
            Some(pairs) => pairs,
            None => return Err(gap(file, "a call whose options splat (`**opts`) is not composed")),
        },
        None => Vec::new(),
    };
    let mut added: Vec<&MergedOption> = Vec::new();
    for option in options {
        match own.iter().find(|o| o.key == option.key) {
            Some(existing) if existing.hash_value && option.hash_value => {
                return Err(gap(file, &format!("deep-merging two hashes under `{}:` is not composed", option.key)));
            }
            Some(_) => {}
            None => added.push(option),
        }
    }
    // Checked over the final per-call set, so an outer condition the
    // call's own options shadow never declines.
    if checked_if {
        for option in own.iter().chain(added.iter().copied()) {
            if matches!(option.key.as_str(), "if" | "unless") && !option.cond_ok {
                return Err(gap(
                    file,
                    &format!("a merged `{}:` that is not a Symbol or a lambda/proc is not composed", option.key),
                ));
            }
        }
    }
    if added.is_empty() && receiver_len == 0 {
        return Ok(None);
    }
    let added_src = added.iter().map(|o| o.source.as_str()).collect::<Vec<_>>().join(", ");
    let (offset, text) = if added.is_empty() {
        (call.location().end_offset(), String::new())
    } else {
        match args.last() {
            Some(last) => (last.location().end_offset(), format!(", {added_src}")),
            None => match call.closing_loc() {
                Some(close) => (close.start_offset(), added_src),
                None => match call.message_loc() {
                    Some(name) => (name.end_offset(), format!(" {added_src}")),
                    None => return Err(gap(file, "call has no name to merge into")),
                },
            },
        }
    };
    let location = call.location();
    let start = location.start_offset();
    let original = location.as_slice();
    // Pad to the call's own offset so diagnostics still point into the
    // file; only what follows the insertion point shifts.
    let mut source = vec![b' '; start];
    source.extend(std::iter::repeat_n(b' ', receiver_len));
    source.extend_from_slice(&original[receiver_len..offset - start]);
    source.extend_from_slice(text.as_bytes());
    source.extend_from_slice(&original[offset - start..]);
    Ok(Some(source))
}
