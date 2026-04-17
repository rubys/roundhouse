//! ERB template compiler.
//!
//! Compiles ERB source to an equivalent Ruby source program that builds up
//! a string via `_buf = _buf + ...` operations. The compiled Ruby is then
//! parsed by Prism and ingested through the existing IR pipeline, so
//! control flow inside `<% %>` tags becomes regular Ruby AST nodes.
//!
//! Design note: we intentionally use `_buf = _buf + X` rather than
//! `_buf += X` so the ingester only needs `LocalVariableWriteNode` (which
//! it already handles) rather than `LocalVariableOperatorWriteNode`.
//!
//! Block-expression output tags (`<%= form_with(x) do |f| %>...<% end %>`)
//! are detected with the BLOCK_EXPR regex from ruby2js/railcar. For those,
//! the compiler emits `_buf = _buf + (EXPR` (no `.to_s`, no closing paren)
//! and defers the close to the matching `<% end %>`, which emits `end).to_s`.
//! A small block stack matches each `<% end %>` to its opener.

/// Classification of a block on the compile-time stack. `Output` means the
/// block was opened by a `<%= ... do %>` tag and must close with
/// `end).to_s` to complete the enclosing `_buf = _buf + (EXPR).to_s` form.
/// `Pass` is any other block (if/while/each do/etc.) — its `<% end %>`
/// stays a plain `end`, since Ruby stitches the passthrough code normally.
#[derive(Clone, Copy)]
enum BlockKind {
    Output,
    Pass,
}

/// Compile ERB source to Ruby source. The compiled Ruby is a sequence of
/// statements suitable for parsing as a `ProgramNode`'s body.
///
/// Text chunks are accumulated in a buffer and flushed only when a
/// meaningful tag (output or code) is about to be emitted. `<%# comment %>`
/// tags are dropped entirely without flushing, so the text surrounding a
/// comment merges into one chunk — this is what lets IR round-trip
/// identical across ingest → emit → ingest when comments (which today
/// drop silently) are present.
pub fn compile_erb(source: &str) -> String {
    let mut out = String::new();
    out.push_str("_buf = \"\"\n");
    let mut stack: Vec<BlockKind> = Vec::new();
    let mut pending_text = String::new();

    let bytes = source.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        match find_at(bytes, cursor, b"<%") {
            None => {
                pending_text.push_str(&source[cursor..]);
                break;
            }
            Some(open) => {
                if open > cursor {
                    pending_text.push_str(&source[cursor..open]);
                }
                let is_output = bytes.get(open + 2) == Some(&b'=');
                let is_comment = !is_output && bytes.get(open + 2) == Some(&b'#');
                let body_start = if is_output { open + 3 } else { open + 2 };
                let close = find_at(bytes, body_start, b"%>")
                    .expect("unterminated ERB tag");
                let ruby = source[body_start..close].trim();
                if is_comment {
                    // Comment tag — intentionally drop without flushing, so
                    // surrounding text chunks merge into one string literal.
                } else if is_output {
                    flush_text(&mut pending_text, &mut out);
                    if is_block_expr(ruby) {
                        // `<%= EXPR do |p| %>` — open an output-block. The
                        // enclosing paren and `.to_s` are emitted on the
                        // matching `<% end %>` tag.
                        out.push_str("_buf = _buf + (");
                        out.push_str(ruby);
                        out.push('\n');
                        stack.push(BlockKind::Output);
                    } else {
                        // Wrap in parens so bareword-arg calls
                        // (`link_to x, y, class: "..."`) and low-precedence
                        // operators (`a || b`) bind as a single expression.
                        // Ingest unwraps ParenthesesNode transparently.
                        out.push_str("_buf = _buf + (");
                        out.push_str(ruby);
                        out.push_str(").to_s\n");
                    }
                } else if ruby == "end" {
                    flush_text(&mut pending_text, &mut out);
                    match stack.pop() {
                        Some(BlockKind::Output) => out.push_str("end).to_s\n"),
                        _ => out.push_str("end\n"),
                    }
                } else {
                    // `<% code %>` — passthrough. Track block openers so
                    // their matching `<% end %>` stays a plain `end`.
                    flush_text(&mut pending_text, &mut out);
                    out.push_str(ruby);
                    out.push('\n');
                    if opens_passthrough_block(ruby) {
                        stack.push(BlockKind::Pass);
                    }
                }
                cursor = close + 2;
            }
        }
    }

    flush_text(&mut pending_text, &mut out);
    out.push_str("_buf\n");
    out
}

fn flush_text(pending: &mut String, out: &mut String) {
    if !pending.is_empty() {
        out.push_str("_buf = _buf + ");
        out.push_str(&ruby_string_literal(pending));
        out.push('\n');
        pending.clear();
    }
}

/// Does `code` end in a block opener (`do`, `do |p|`, `{`, `{ |p| `)?
/// Mirrors ruby2js's `BLOCK_EXPR = /((\s|\))do|\{)(\s*\|[^|]*\|)?\s*\z/`.
fn is_block_expr(code: &str) -> bool {
    let code = code.trim_end();
    // If the tail is `|params|`, strip it and keep checking the prefix.
    let prefix = if let Some(without_trailing_bar) = code.strip_suffix('|') {
        match without_trailing_bar.rfind('|') {
            Some(p) => without_trailing_bar[..p].trim_end(),
            None => return false,
        }
    } else {
        code
    };
    if prefix.ends_with('{') {
        return true;
    }
    if prefix == "do" {
        return true;
    }
    // `do` must follow whitespace or `)` to avoid matching identifiers that
    // happen to end in "do" (e.g., `redo`).
    if let Some(stripped) = prefix.strip_suffix("do") {
        let last = stripped.chars().last();
        return matches!(last, Some(c) if c.is_whitespace() || c == ')');
    }
    false
}

/// Does `code` (inside a `<% code %>` tag) open a block whose `end` we
/// need to track? Covers the control-flow keywords plus method calls with
/// trailing `do`/`{`. Middle markers (`else`, `elsif ...`, `when ...`,
/// `rescue`, `ensure`, `in ...`) do NOT open a new block.
fn opens_passthrough_block(code: &str) -> bool {
    let t = code.trim();
    if t.is_empty() {
        return false;
    }
    for opener in &["if", "unless", "while", "until", "for", "case", "begin",
                    "class", "def", "module"] {
        if t == *opener
            || t.starts_with(&format!("{opener} "))
            || t.starts_with(&format!("{opener}("))
        {
            return true;
        }
    }
    is_block_expr(t)
}

fn ruby_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn find_at(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || start >= bytes.len() {
        return None;
    }
    bytes[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_text_only() {
        let out = compile_erb("<h1>Hi</h1>\n");
        assert!(out.contains(r#"_buf = _buf + "<h1>Hi</h1>\n""#));
    }

    #[test]
    fn output_interpolation() {
        let out = compile_erb("Total: <%= count %>\n");
        assert!(out.contains(r#"_buf = _buf + "Total: ""#));
        assert!(out.contains("_buf = _buf + (count).to_s"));
    }

    #[test]
    fn detects_block_expr() {
        assert!(is_block_expr("form_with(x) do |f|"));
        assert!(is_block_expr("form_with(x) do"));
        assert!(is_block_expr("items.each do |n|"));
        assert!(is_block_expr("foo {"));
        assert!(is_block_expr("foo { |n|"));
        assert!(!is_block_expr("x.do_something"));
        assert!(!is_block_expr("redo"));
        assert!(!is_block_expr("form_with(x)"));
    }

    #[test]
    fn output_block_tag_compiles_with_matching_end() {
        let src = "<%= form_with(x) do |f| %>inner<% end %>";
        let out = compile_erb(src);
        assert!(
            out.contains("_buf = _buf + (form_with(x) do |f|"),
            "compiled:\n{out}"
        );
        assert!(out.contains("end).to_s"), "compiled:\n{out}");
    }

    #[test]
    fn passthrough_block_end_stays_plain() {
        let src = "<% if cond %>text<% end %>";
        let out = compile_erb(src);
        assert!(out.contains("if cond"));
        // Should emit plain `end`, not `end).to_s`.
        assert!(out.contains("\nend\n"), "compiled:\n{out}");
        assert!(!out.contains("end).to_s"), "compiled:\n{out}");
    }

    #[test]
    fn nested_output_and_passthrough_close_in_order() {
        let src = "<%= form_with(x) do |f| %><% if cond %>x<% end %><% end %>";
        let out = compile_erb(src);
        // Inner end closes if; outer end closes form_with (output-block).
        assert!(out.contains("\nend\n"), "compiled:\n{out}");
        assert!(out.contains("end).to_s"), "compiled:\n{out}");
        // `end).to_s` must appear AFTER the plain `end` for `if`.
        let plain_idx = out.find("\nend\n").unwrap();
        let close_idx = out.find("end).to_s").unwrap();
        assert!(plain_idx < close_idx, "close order wrong:\n{out}");
    }
}
