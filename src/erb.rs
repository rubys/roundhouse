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

/// `Some("")` for a bare `end` tag, `Some("if cond")` for a block closed
/// by a trailing modifier, `None` for anything else.
///
/// Only the modifiers that can legally follow a block's `end` — the same
/// set Ruby accepts as statement modifiers.
fn end_tag_modifier(ruby: &str) -> Option<&str> {
    if ruby == "end" {
        return Some("");
    }
    let rest = ruby.strip_prefix("end")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let is_modifier = ["if ", "unless ", "while ", "until "]
        .iter()
        .any(|kw| rest.starts_with(kw));
    is_modifier.then_some(rest)
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
    // Erubi's `is_bol`: did the PREVIOUS tag end its line? Seeds the
    // `lspace` test for a tag whose preceding text chunk is empty.
    // Starts true — the template's first byte is a line start.
    let mut is_bol = true;

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
                // Erubi's raw-output tag `<%== expr %>`: same output
                // position as `<%=`, but skipping auto-escape. The
                // escape decision lives downstream (the view walker's
                // helper/partial/yield carve-outs); here we only need
                // the tag body to start after BOTH equals signs so the
                // code parses as plain Ruby.
                let is_raw_output = is_output && bytes.get(open + 3) == Some(&b'=');
                let is_comment = !is_output && bytes.get(open + 2) == Some(&b'#');
                // Erubi trim markers: under Rails' default trim mode
                // `<%-` behaves exactly like `<%`, and the `-` of a
                // closing `-%>` is likewise just a marker — strip both
                // so the tag body parses as plain Ruby. (Line-level
                // whitespace handling for code tags already lives in
                // `erubi_trim_body` at target-emit time.)
                let is_trim_open =
                    !is_output && !is_comment && bytes.get(open + 2) == Some(&b'-');
                let body_start = if is_raw_output {
                    open + 4
                } else if is_output || is_trim_open {
                    open + 3
                } else {
                    open + 2
                };
                let close = find_at(bytes, body_start, b"%>")
                    .expect("unterminated ERB tag");
                let body = &source[body_start..close];
                // Erubi's `tailch` — the `-`/`=` before `%>`. Only an
                // OUTPUT tag acts on it (`<%= x -%>` drops the newline);
                // for a code tag the trim decision is lspace/rspace
                // alone, so `<% x -%>` mid-line keeps its newline.
                let had_tail_dash = body.ends_with('-');
                let body = body.strip_suffix('-').unwrap_or(body);
                let ruby = body.trim();

                // Erubi's `rspace`: the optional `[ \t]*\r?\n` right
                // after `%>`. Absent (None) when the tag has trailing
                // non-whitespace on its line.
                let after_tag = close + 2;
                let rspace_len = {
                    let mut j = after_tag;
                    while matches!(bytes.get(j), Some(b' ') | Some(b'\t')) {
                        j += 1;
                    }
                    if bytes.get(j) == Some(&b'\r') {
                        j += 1;
                    }
                    if bytes.get(j) == Some(&b'\n') {
                        Some(j + 1 - after_tag)
                    } else {
                        None
                    }
                };
                // Erubi's `lspace`: the run between the last newline and
                // this tag, when it is entirely spaces/tabs. Never
                // computed for output tags — `<%= %>` keeps its indent.
                let lspace_len: Option<usize> = if is_output {
                    None
                } else if pending.text.is_empty() {
                    is_bol.then_some(0)
                } else if pending.text.ends_with('\n') {
                    Some(0)
                } else {
                    let ws = |s: &str| s.bytes().all(|b| b == b' ' || b == b'\t');
                    match pending.text.rfind('\n') {
                        Some(p) => {
                            let tail = &pending.text[p + 1..];
                            ws(tail).then(|| tail.len())
                        }
                        None => (is_bol && ws(&pending.text))
                            .then(|| pending.text.len()),
                    }
                };
                // THE trim rule (erubi's `nil`/`-`/`#` arms): a code or
                // comment tag alone on its line contributes nothing to
                // the output — its indentation AND its newline both
                // vanish. This is what Rails renders, and not doing it
                // leaked `indent + "\n"` per control-flow tag into every
                // page: lobsters `/u` came out 292,443 bytes against
                // Rails' 173,576, a 68% gap that was entirely this.
                let trims = lspace_len.is_some() && rspace_len.is_some();
                if trims {
                    let keep = pending.text.len() - lspace_len.unwrap();
                    pending.text.truncate(keep);
                    if pending.text.is_empty() {
                        pending.range = None;
                    }
                }
                is_bol = rspace_len.is_some();
                if is_comment {
                    // Comment tag — intentionally drop without flushing, so
                    // surrounding text chunks merge into one string literal.
                    // Its line-level whitespace was already handled by the
                    // shared `trims` rule above, which erubi applies to
                    // comment and code tags identically.
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
                } else if let Some(modifier) = end_tag_modifier(ruby) {
                    // `<% end %>`, and `<% end if cond %>` — a block
                    // closed by a trailing modifier. Matching `"end"`
                    // exactly sent the modifier form down the passthrough
                    // arm, which never popped the block stack and never
                    // emitted an Output block's `).to_s` close, so the
                    // translation didn't parse (prism `MissingNode`).
                    //
                    // The modifier rides INSIDE the parens: `(expr if
                    // cond)` is nil when the condition is false and
                    // `nil.to_s` is "", which is what Rails renders for a
                    // skipped block.
                    pending.flush(&mut out, &mut map);
                    record_code(&mut map, out.len(), ruby, body_start, body);
                    let tail = if modifier.is_empty() {
                        "end".to_string()
                    } else {
                        format!("end {modifier}")
                    };
                    match stack.pop() {
                        Some(BlockKind::Output) => out.push_str(&format!("{tail}).to_s\n")),
                        _ => out.push_str(&format!("{tail}\n")),
                    }
                } else {
                    // `<% code %>` — passthrough. Track block openers so
                    // their matching `<% end %>` stays a plain `end`.
                    //
                    // Case-arm continuation tags (`<% when X %>`,
                    // `<% in X %>`): a whitespace-only chunk pending
                    // between `<% case x %>` and its first arm would
                    // flush as a `_buf` append *between* the scrutinee
                    // and `when` — invalid Ruby. Erubi's trim mode
                    // drops such whitespace-only lines; mirror it here
                    // for the arm keywords so the translation parses.
                    if (ruby.starts_with("when ") || ruby.starts_with("in "))
                        && pending.text.bytes().all(|b| b.is_ascii_whitespace())
                    {
                        pending.text.clear();
                        pending.range = None;
                    }
                    pending.flush(&mut out, &mut map);
                    record_code(&mut map, out.len(), ruby, body_start, body);
                    out.push_str(ruby);
                    out.push('\n');
                    if opens_passthrough_block(ruby) {
                        stack.push(BlockKind::Pass);
                    }
                }
                cursor = after_tag;
                // Consume the tag's trailing newline when the trim rule
                // fired (code/comment tag alone on its line), or when an
                // OUTPUT tag closed with `-%>` — erubi's
                // `rspace = nil if tailch`, the one case where the tail
                // marker does the work rather than lspace/rspace.
                if trims || (is_output && had_tail_dash) {
                    cursor += rspace_len.unwrap_or(0);
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
pub(crate) fn is_block_expr(code: &str) -> bool {
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
pub(crate) fn opens_passthrough_block(code: &str) -> bool {
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

pub(crate) fn ruby_string_literal(s: &str) -> String {
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
    fn trim_marker_tags_strip_to_plain_code() {
        // `<%- ... -%>` — erubi trim markers are not part of the Ruby.
        let src = "<%- raise \"x\" if bad -%>\ndone";
        let (compiled, _) = compile_erb_mapped(src);
        assert!(
            compiled.contains("raise \"x\" if bad\n"),
            "trim markers stripped from tag body:\n{compiled}"
        );
        assert!(!compiled.contains("- raise"), "leading `-` must not survive:\n{compiled}");
    }

    /// Concatenate just the TEXT chunks of a compiled template — the
    /// `_buf = _buf + "..."` literals, unescaped. This is the rendered
    /// output minus every `<%= %>` substitution, which is exactly the
    /// surface the trim rule governs.
    fn static_text(src: &str) -> String {
        let compiled = compile_erb(src);
        let mut out = String::new();
        for line in compiled.lines() {
            let Some(rest) = line.strip_prefix("_buf = _buf + \"") else {
                continue;
            };
            let Some(body) = rest.strip_suffix('"') else { continue };
            let mut it = body.chars();
            while let Some(c) = it.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                }
            }
        }
        out
    }

    #[test]
    fn code_tag_alone_on_a_line_leaves_no_whitespace() {
        // Erubi's trim rule: a `<% %>` tag whose line holds nothing else
        // contributes NOTHING — its indentation and its newline both go.
        // Verified byte-for-byte against erubi 1.13.1, which renders
        // this template as exactly `<div>\n    <p>hi</p>\n</div>\n`.
        //
        // Not doing this leaked `indent + "\n"` per control-flow tag
        // into every rendered page; across lobsters' 89 templates it was
        // 4,261 bytes of static text, and it multiplies by iteration
        // count inside loops — lobsters `/u` rendered 292,443 bytes
        // against Rails' 173,576, a 68% gap that was entirely this.
        let src = "<div>\n  <% if true %>\n    <p>hi</p>\n  <% end %>\n</div>\n";
        assert_eq!(static_text(src), "<div>\n    <p>hi</p>\n</div>\n");
    }

    #[test]
    fn mid_line_code_tag_keeps_its_whitespace() {
        // The counterweight: trimming needs BOTH a whitespace-only left
        // side and a newline on the right. A tag with real text beside
        // it has neither, so every byte around it survives — erubi
        // renders this as `<p>a  b</p>\n`.
        assert_eq!(static_text("<p>a <% x = 1 %> b</p>\n"), "<p>a  b</p>\n");
    }

    #[test]
    fn output_tag_keeps_indent_but_honors_tail_dash() {
        // `<%= %>` never left-trims (erubi computes no lspace for it),
        // so the indent before it is real output. The `-%>` marker is
        // the one case where the tail does the work: it drops the
        // trailing newline on its own.
        assert_eq!(static_text("  <%= v %>\nx\n"), "  \nx\n");
        assert_eq!(static_text("x<%= 7 -%>\ny\n"), "xy\n");
    }

    #[test]
    fn case_arms_split_across_tags_stay_parseable() {
        // Whitespace between `<% case x %>` and `<% when A %>` must not
        // flush as a `_buf` append (invalid Ruby between scrutinee and
        // first arm) — erubi trim mode drops it; so do we.
        let src = "<% case x %>\n  <% when A %>a<% when B %>b<% end %>";
        let (compiled, _) = compile_erb_mapped(src);
        assert!(
            compiled.contains("case x\nwhen A\n"),
            "no buffer append between case and first when:\n{compiled}"
        );
        ruby_prism::parse(compiled.as_bytes())
            .errors()
            .next()
            .map(|e| panic!("translated case/when template must parse: {e:?}\n{compiled}"));
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
    fn end_with_a_trailing_modifier_still_closes_its_block() {
        // `<% end if cond %>` — a block closed by a statement modifier.
        // Matching the tag against `"end"` EXACTLY sent this down the
        // passthrough arm: the block stack never popped and the output
        // block's `).to_s` close was never emitted, so the translation
        // didn't parse at all (prism `MissingNode`).
        let src = "<%= wrap(x) do %>hi<% end if cond %>";
        let out = compile_erb(src);
        // The modifier rides INSIDE the parens: `(expr if cond)` is nil
        // when the condition is false, and `nil.to_s` is "" — which is
        // what Rails renders for a skipped block.
        assert!(out.contains("end if cond).to_s"), "compiled:\n{out}");
    }

    #[test]
    fn end_with_a_modifier_on_a_passthrough_block_stays_a_plain_end() {
        let src = "<% items.each do |i| %>x<% end unless skip %>";
        let out = compile_erb(src);
        assert!(out.contains("end unless skip"), "compiled:\n{out}");
        assert!(!out.contains(").to_s"), "compiled:\n{out}");
    }

    #[test]
    fn a_method_named_ending_is_not_an_end_tag() {
        // The prefix test needs the whitespace check — `ending` and
        // `end_of_day` both start with "end".
        assert_eq!(end_tag_modifier("ending"), None);
        assert_eq!(end_tag_modifier("end_of_day"), None);
        assert_eq!(end_tag_modifier("end"), Some(""));
        assert_eq!(end_tag_modifier("end if cond"), Some("if cond"));
        // Only real statement modifiers — `end foo` is not one.
        assert_eq!(end_tag_modifier("end foo"), None);
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
