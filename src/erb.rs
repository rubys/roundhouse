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
//! Current scope: text + `<%= expr %>` output interpolation.
//! Deferred: `<% code %>` control flow (loops/conditionals), trim markers
//! (`<%-` / `-%>`), ERB comments (`<%# %>`), unescaped output (`<%== %>`).

/// Compile ERB source to Ruby source. The compiled Ruby is a sequence of
/// statements suitable for parsing as a `ProgramNode`'s body.
pub fn compile_erb(source: &str) -> String {
    let mut out = String::new();
    out.push_str("_buf = \"\"\n");

    let bytes = source.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        match find_at(bytes, cursor, b"<%") {
            None => {
                let text = &source[cursor..];
                if !text.is_empty() {
                    append_text(&mut out, text);
                }
                break;
            }
            Some(open) => {
                if open > cursor {
                    append_text(&mut out, &source[cursor..open]);
                }
                let is_output = bytes.get(open + 2) == Some(&b'=');
                let body_start = if is_output { open + 3 } else { open + 2 };
                let close = find_at(bytes, body_start, b"%>")
                    .expect("unterminated ERB tag");
                let ruby = source[body_start..close].trim();
                if is_output {
                    // Deliberately omit wrapping parens for MVP — simpler
                    // expressions (`@posts.length`) don't need them, and
                    // our ingester doesn't yet handle ParenthesesNode.
                    // When a fixture with a low-precedence operator
                    // (`a || b`) lands, wrap in parens and add the
                    // ParenthesesNode case to ingest_expr.
                    out.push_str("_buf = _buf + ");
                    out.push_str(ruby);
                    out.push_str(".to_s\n");
                } else {
                    // `<% code %>` — control flow. Deferred for this pass;
                    // emit raw but callers should only provide text + output
                    // fragments until control-flow support lands.
                    out.push_str(ruby);
                    out.push('\n');
                }
                cursor = close + 2;
            }
        }
    }

    out.push_str("_buf\n");
    out
}

fn append_text(out: &mut String, text: &str) {
    out.push_str("_buf = _buf + ");
    out.push_str(&ruby_string_literal(text));
    out.push('\n');
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
        assert!(out.contains("_buf = _buf + count.to_s"));
    }
}
