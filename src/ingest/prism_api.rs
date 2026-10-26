//! Thin adapters over the `ruby-prism` crate surface Roundhouse touches.
//!
//! Prism 2.0.0 renamed Location offsets (`start_offset` → `start` / `end`),
//! made `Diagnostic::message` return `&[u8]`, and replaced `MissingNode`
//! with `ErrorRecoveryNode`.
//!
//! Call-site style: use [`PrismLocation`] for offsets (`loc.start_offset()`).
//! Free functions remain the trait's implementation plus non-method helpers
//! (`span_offsets`, `diagnostic_message`, `recovery_unsupported_message`,
//! `block_parameter`). Prefer native `start`/`end` only when rewriting a
//! site deliberately — not as a mixed style alongside the trait.

use ruby_prism::{BlockParameterNode, Diagnostic, ErrorRecoveryNode, Location, ParametersNode};

/// Byte offset where `loc` starts in the parsed source.
#[inline]
pub(crate) fn start_offset(loc: &Location<'_>) -> usize {
    loc.start() as usize
}

/// Byte offset where `loc` ends in the parsed source.
#[inline]
pub(crate) fn end_offset(loc: &Location<'_>) -> usize {
    loc.end() as usize
}

/// `(start, end)` as the `u32` pair [`crate::span::Span`] stores.
#[inline]
pub(crate) fn span_offsets(loc: &Location<'_>) -> (u32, u32) {
    (start_offset(loc) as u32, end_offset(loc) as u32)
}

/// Human-readable text of a Prism diagnostic.
///
/// Prism 2.0 returns `&[u8]` (source encoding; not always UTF-8). Lossy
/// conversion lives here so callers keep a `String`.
#[inline]
pub(crate) fn diagnostic_message(err: &Diagnostic<'_>) -> String {
    String::from_utf8_lossy(err.message()).into_owned()
}

/// Unsupported-ingest message for a Prism recovery node, including a short
/// source preview. Prefer Prism's `unexpected()` span when present; otherwise
/// fall back to the recovery node's own location (common for missing-token
/// placeholders where `unexpected` is null).
pub(crate) fn recovery_unsupported_message(recovery: &ErrorRecoveryNode<'_>) -> String {
    let bytes = recovery
        .unexpected()
        .map(|unexpected| unexpected.location().as_slice())
        .unwrap_or_else(|| recovery.location().as_slice());
    // Cap before lossy decode so a huge recovered call/pattern span does
    // not allocate a full UTF-8 buffer just to take 40 chars.
    let capped = &bytes[..bytes.len().min(160)];
    let preview = escape_recovery_preview(String::from_utf8_lossy(capped).chars());
    if preview.is_empty() {
        "unparsed fragment (Prism recovery node)".into()
    } else {
        format!("unparsed fragment near `{preview}` (Prism recovery)")
    }
}

/// Keep recovery previews on one diagnostic line: escape newlines and other
/// controls after the byte cap / 40-char display limit, not before.
fn escape_recovery_preview(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for (i, c) in chars.enumerate() {
        if i >= 40 {
            break;
        }
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Line/paragraph separators are not Cc (`is_control`), but still
            // break a one-line diagnostic in many terminals.
            '\u{2028}' => out.push_str("\\u{2028}"),
            '\u{2029}' => out.push_str("\\u{2029}"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

/// Prism 2 returns `ParametersNode::block` as a generic [`Node`] (`&blk`,
/// anonymous `&`, or `&nil`). Narrow to the named/anonymous block-parameter
/// form callers already expect.
#[inline]
pub(crate) fn block_parameter<'pr>(
    parameters: &ParametersNode<'pr>,
) -> Option<BlockParameterNode<'pr>> {
    parameters.block()?.as_block_parameter_node()
}

/// Method-shaped offset aliases for Prism 2's `start` / `end`. Bring into
/// scope with `use …::PrismLocation`.
pub(crate) trait PrismLocation {
    fn start_offset(&self) -> usize;
    fn end_offset(&self) -> usize;
}

impl PrismLocation for Location<'_> {
    #[inline]
    fn start_offset(&self) -> usize {
        start_offset(self)
    }

    #[inline]
    fn end_offset(&self) -> usize {
        end_offset(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruby_prism::Node;

    #[test]
    fn offsets_match_source_slice() {
        let result = ruby_prism::parse(b"abc");
        let loc = result.node().location();
        assert_eq!(start_offset(&loc), 0);
        assert_eq!(end_offset(&loc), 3);
        assert_eq!(span_offsets(&loc), (0, 3));
        assert_eq!(loc.start_offset(), 0);
        assert_eq!(loc.end_offset(), 3);
        assert_eq!(loc.as_slice(), b"abc");
    }

    #[test]
    fn recovery_node_on_truncated_source() {
        let result = ruby_prism::parse(b"1 + ");
        assert!(result.errors().next().is_some());
        struct Finder<'pr> {
            hit: Option<Node<'pr>>,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Finder<'pr> {
            fn visit_error_recovery_node(
                &mut self,
                node: &ruby_prism::ErrorRecoveryNode<'pr>,
            ) {
                self.hit = Some(node.as_node());
            }
        }
        let mut finder = Finder { hit: None };
        ruby_prism::Visit::visit(&mut finder, &result.node());
        let node = finder
            .hit
            .expect("expected a Prism recovery node for truncated `1 + `");
        let recovery = node
            .as_error_recovery_node()
            .expect("hit must narrow to ErrorRecoveryNode");
        let message = recovery_unsupported_message(&recovery);
        assert!(
            message.contains("Prism recovery") && message.contains('`'),
            "expected a preview-bearing recovery message, got: {message}"
        );
    }

    #[test]
    fn diagnostic_message_is_non_empty() {
        let result = ruby_prism::parse(b"def\n");
        let err = result.errors().next().expect("syntax error");
        assert!(!diagnostic_message(&err).is_empty());
    }

    #[test]
    fn recovery_preview_escapes_controls() {
        let escaped = escape_recovery_preview("a\nb\tc\u{7}d\u{2028}e".chars());
        assert_eq!(escaped, "a\\nb\\tc\\u{7}d\\u{2028}e");
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\u{7}'));
        assert!(!escaped.contains('\u{2028}'));
        // Display limit is 40 source chars (escaping may lengthen the string).
        let mixed = format!("{}\n{}", "a".repeat(20), "b".repeat(30));
        let escaped = escape_recovery_preview(mixed.chars());
        assert_eq!(escaped.matches('a').count(), 20);
        assert!(escaped.contains("\\n"));
        assert_eq!(escaped.matches('b').count(), 19); // 20 + newline + 19 = 40
        assert!(!escaped.contains('\n'));
    }

    fn first_def<'pr>(result: &'pr ruby_prism::ParseResult<'_>) -> ruby_prism::DefNode<'pr> {
        let program = result.node().as_program_node().expect("program");
        program
            .statements()
            .body()
            .iter()
            .next()
            .expect("stmt")
            .as_def_node()
            .expect("def")
    }

    #[test]
    fn block_parameter_narrows_named_and_anonymous() {
        let named = ruby_prism::parse(b"def f(&blk); end");
        let pn = first_def(&named).parameters().expect("params");
        let bp = block_parameter(&pn).expect("block param");
        assert_eq!(bp.name().map(|n| n.as_slice()), Some(&b"blk"[..]));

        let anon = ruby_prism::parse(b"def f(&); end");
        let pn = first_def(&anon).parameters().expect("params");
        let bp = block_parameter(&pn).expect("anonymous block param");
        assert!(bp.name().is_none());

        // Ruby 4.1 `&nil` is a different node kind — not a block parameter.
        let nil_block = ruby_prism::parse(b"def f(&nil); end");
        let pn = first_def(&nil_block).parameters().expect("params");
        assert!(block_parameter(&pn).is_none());
    }
}
