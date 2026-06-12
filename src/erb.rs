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

/// One compiled-Ruby ↔ template byte-range correspondence out of
/// [`compile_erb_mapped`]. Tag bodies are copied into the compiled
/// output byte-for-byte, so offsets inside those segments translate
/// exactly; text-chunk segments cover the emitted string literal
/// (whose escaping changes lengths), so their endpoints are exact and
/// interior offsets clamp. Segments are ordered and non-overlapping
/// in both coordinate spaces.
#[derive(Clone, Copy, Debug)]
pub struct ErbSegment {
    /// Byte range in the compiled Ruby.
    pub c_start: u32,
    pub c_end: u32,
    /// Corresponding byte range in the original template.
    pub e_start: u32,
    pub e_end: u32,
}

/// Translate one compiled-Ruby byte offset to a template byte offset.
///
/// Offsets inside a segment translate by delta (clamped to the
/// segment's template range — exact for tag code, endpoint-exact for
/// escaped text literals). Offsets in the synthesized glue between
/// segments (`_buf = _buf + (`, `).to_s`, the prologue/epilogue) snap
/// to the end of the preceding segment — i.e. a statement span that
/// starts at the `_buf` before a tag's code lands on the tag itself.
/// Monotonic, so translated spans stay well-formed.
pub fn translate_offset(map: &[ErbSegment], o: u32) -> u32 {
    let idx = map.partition_point(|s| s.c_start <= o);
    if idx == 0 {
        return map.first().map(|s| s.e_start).unwrap_or(0);
    }
    let seg = &map[idx - 1];
    if o <= seg.c_end {
        (seg.e_start + (o - seg.c_start)).min(seg.e_end)
    } else {
        seg.e_end
    }
}

/// Rewrite every real span in `e` (recursively) from compiled-Ruby
/// offsets to template offsets via `map`. Synthetic spans pass through
/// untouched. Run once on a view body right after ingest, before any
/// lowering clones spans around.
pub fn translate_spans(e: &mut crate::expr::Expr, map: &[ErbSegment]) {
    if !e.span.is_synthetic() {
        e.span.start = translate_offset(map, e.span.start);
        e.span.end = translate_offset(map, e.span.end).max(e.span.start);
    }
    e.node.for_each_child_mut(&mut |c| translate_spans(c, map));
}

/// Pending text-chunk accumulator: the literal text plus the template
/// byte range it came from. Chunks merge across `<%# comment %>` tags
/// (round-trip fidelity — see `compile_erb_mapped`), in which case the
/// range covers the comment too.
#[derive(Default)]
struct PendingText {
    text: String,
    /// Template range of the accumulated text; `None` while empty.
    range: Option<(usize, usize)>,
}

impl PendingText {
    fn push(&mut self, slice: &str, e_start: usize, e_end: usize) {
        if slice.is_empty() {
            return;
        }
        self.text.push_str(slice);
        self.range = Some((self.range.map(|(s, _)| s).unwrap_or(e_start), e_end));
    }

    fn flush(&mut self, out: &mut String, map: &mut Vec<ErbSegment>) {
        if self.text.is_empty() {
            self.range = None;
            return;
        }
        let (e_start, e_end) = self.range.take().expect("non-empty pending has a range");
        out.push_str("_buf = _buf + ");
        let c_start = out.len();
        out.push_str(&ruby_string_literal(&self.text));
        map.push(ErbSegment {
            c_start: c_start as u32,
            c_end: out.len() as u32,
            e_start: e_start as u32,
            e_end: e_end as u32,
        });
        out.push('\n');
        self.text.clear();
    }
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
    compile_erb_mapped(source).0
}

/// As [`compile_erb`], plus the segment table that maps compiled-Ruby
/// byte ranges back to template byte ranges (see [`ErbSegment`]).
/// View ingest uses it to translate spans so diagnostics and source
/// maps report template positions, not compiled-Ruby ones.
pub fn compile_erb_mapped(source: &str) -> (String, Vec<ErbSegment>) {
    let mut out = String::new();
    let mut map: Vec<ErbSegment> = Vec::new();
    out.push_str("_buf = \"\"\n");
    let mut stack: Vec<BlockKind> = Vec::new();
    let mut pending = PendingText::default();

    // Record the byte-identical copy of a tag's (trimmed) code into
    // the segment table. `body_start..close` is the untrimmed tag
    // body in the template; `ruby` is its trim; `c_start` is where the
    // copy landed in `out`.
    let record_code =
        |map: &mut Vec<ErbSegment>, c_start: usize, ruby: &str, body_start: usize, body: &str| {
            let lead = body.len() - body.trim_start().len();
            let e_start = body_start + lead;
            map.push(ErbSegment {
                c_start: c_start as u32,
                c_end: (c_start + ruby.len()) as u32,
                e_start: e_start as u32,
                e_end: (e_start + ruby.len()) as u32,
            });
        };

    let bytes = source.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        match find_at(bytes, cursor, b"<%") {
            None => {
                pending.push(&source[cursor..], cursor, source.len());
                break;
            }
            Some(open) => {
                if open > cursor {
                    pending.push(&source[cursor..open], cursor, open);
                }
                let is_output = bytes.get(open + 2) == Some(&b'=');
                let is_comment = !is_output && bytes.get(open + 2) == Some(&b'#');
                let body_start = if is_output { open + 3 } else { open + 2 };
                let close = find_at(bytes, body_start, b"%>")
                    .expect("unterminated ERB tag");
                let body = &source[body_start..close];
                let ruby = body.trim();
                if is_comment {
                    // Comment tag — intentionally drop without flushing, so
                    // surrounding text chunks merge into one string literal.
                    //
                    // Rails' erubi trim mode also strips the leading
                    // horizontal whitespace on the comment's line (the
                    // `    ` indent before `<%# ... %>`). Strip the tail
                    // of pending_text back to the last newline when the
                    // intervening bytes are only spaces/tabs — effectively
                    // making the whole comment line disappear from output.
                    let trim_start = pending.text.rfind('\n').map(|p| p + 1).unwrap_or(0);
                    if pending.text[trim_start..]
                        .bytes()
                        .all(|b| b == b' ' || b == b'\t')
                    {
                        pending.text.truncate(trim_start);
                        if pending.text.is_empty() {
                            pending.range = None;
                        }
                    }
                } else if is_output {
                    pending.flush(&mut out, &mut map);
                    if is_block_expr(ruby) {
                        // `<%= EXPR do |p| %>` — open an output-block. The
                        // enclosing paren and `.to_s` are emitted on the
                        // matching `<% end %>` tag.
                        out.push_str("_buf = _buf + (");
                        record_code(&mut map, out.len(), ruby, body_start, body);
                        out.push_str(ruby);
                        out.push('\n');
                        stack.push(BlockKind::Output);
                    } else {
                        // Wrap in parens so bareword-arg calls
                        // (`link_to x, y, class: "..."`) and low-precedence
                        // operators (`a || b`) bind as a single expression.
                        // Ingest unwraps ParenthesesNode transparently.
                        out.push_str("_buf = _buf + (");
                        record_code(&mut map, out.len(), ruby, body_start, body);
                        out.push_str(ruby);
                        out.push_str(").to_s\n");
                    }
                } else if ruby == "end" {
                    pending.flush(&mut out, &mut map);
                    record_code(&mut map, out.len(), ruby, body_start, body);
                    match stack.pop() {
                        Some(BlockKind::Output) => out.push_str("end).to_s\n"),
                        _ => out.push_str("end\n"),
                    }
                } else {
                    // `<% code %>` — passthrough. Track block openers so
                    // their matching `<% end %>` stays a plain `end`.
                    pending.flush(&mut out, &mut map);
                    record_code(&mut map, out.len(), ruby, body_start, body);
                    out.push_str(ruby);
                    out.push('\n');
                    if opens_passthrough_block(ruby) {
                        stack.push(BlockKind::Pass);
                    }
                }
                cursor = close + 2;
                // ERB comment tags (`<%# ... %>`) consume their
                // trailing newline when rendered by Rails' erubi.
                // The rationale: a comment line contributes nothing
                // visible, and leaving the newline behind would
                // produce a stray blank where the comment was. Non-
                // comment tags (`<% code %>`, `<%= expr %>`) don't
                // trim here — their newlines are significant
                // whitespace between the surrounding text chunks.
                // Matching erubi's behavior for `<% %>` tags
                // happens at target-emit time (`erubi_trim_body`)
                // so the Ruby round-trip preserves source fidelity.
                if is_comment && bytes.get(cursor) == Some(&b'\n') {
                    cursor += 1;
                }
            }
        }
    }

    pending.flush(&mut out, &mut map);
    out.push_str("_buf\n");
    (out, map)
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

    /// Translate a compiled offset and assert it lands on the template
    /// offset where `needle` starts.
    fn assert_maps_to(source: &str, compiled_needle: &str, template_needle: &str) {
        let (compiled, map) = compile_erb_mapped(source);
        let c_off = compiled.find(compiled_needle).unwrap_or_else(|| {
            panic!("compiled output missing {compiled_needle:?}:\n{compiled}")
        }) as u32;
        let e_off = source.find(template_needle).unwrap() as u32;
        assert_eq!(
            translate_offset(&map, c_off),
            e_off,
            "compiled offset of {compiled_needle:?} should map to template offset of {template_needle:?}",
        );
    }

    #[test]
    fn tag_code_maps_exactly() {
        let src = "Total: <%= count %> items\n<% if cond %>x<% end %>";
        assert_maps_to(src, "count", "count");
        assert_maps_to(src, "if cond", "if cond");
        assert_maps_to(src, "end", "end");
    }

    #[test]
    fn text_chunk_endpoints_map_to_template_chunk() {
        let src = "Total: <%= count %>";
        let (compiled, map) = compile_erb_mapped(src);
        // The literal (including quotes) maps onto the template's
        // `Total: ` chunk; its start lands at template offset 0.
        let lit = compiled.find("\"Total: \"").unwrap() as u32;
        assert_eq!(translate_offset(&map, lit), 0);
        // An offset past the literal's template range clamps to the
        // chunk's end (just before `<%=`).
        assert_eq!(translate_offset(&map, lit + 9), 7);
    }

    #[test]
    fn glue_offsets_snap_to_the_preceding_segment_end() {
        let src = "Total: <%= count %>";
        let (compiled, map) = compile_erb_mapped(src);
        // The `_buf` opening the output statement sits in synthesized
        // glue after the text literal — it snaps to the text chunk's
        // template end, i.e. where the tag begins.
        let stmt = compiled.rfind("_buf = _buf + (count)").unwrap() as u32;
        assert_eq!(translate_offset(&map, stmt), 7);
        // Offsets before any segment (the `_buf = ""` prologue) land
        // on the first segment's template start.
        assert_eq!(translate_offset(&map, 0), 0);
    }

    #[test]
    fn translate_offset_is_monotonic() {
        let src = "<h1>Hi</h1>\n<%= a %>mid<% if c %>x<% end %>tail";
        let (compiled, map) = compile_erb_mapped(src);
        let mut last = 0;
        for o in 0..=compiled.len() as u32 {
            let t = translate_offset(&map, o);
            assert!(t >= last, "offset {o}: {t} < {last}");
            last = t;
        }
    }

    #[test]
    fn comment_merged_chunk_covers_both_template_runs() {
        let src = "before\n<%# note %>\nafter<%= x %>";
        let (_, map) = compile_erb_mapped(src);
        // One merged text segment spanning from `before` through
        // `after`, then the `x` code segment.
        assert_eq!(map.len(), 2);
        assert_eq!(map[0].e_start, 0);
        assert_eq!(map[0].e_end as usize, src.find("<%= x %>").unwrap());
    }

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
