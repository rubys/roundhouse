//! HAML template compiler (Mastodon subset — see roundhouse#59).
//!
//! Like [`crate::erb`], this compiles a template to the `_buf`-append Ruby
//! shape (`_buf = ""` … `_buf = _buf + EXPR` … `_buf`) plus a segment map,
//! so the result flows through the shared `ingest_template` view pipeline
//! unchanged. The difference from ERB is structural: HAML is
//! indentation-nested with no explicit `<% end %>`, so the compiler walks
//! lines against an **indentation stack** of open frames, each of which
//! knows how to close when the indentation drops (an element emits its
//! `</tag>`, a Ruby block emits `end`, an output block emits `end).to_s`).
//!
//! Scope is the disciplined subset Mastodon actually uses: `%tag`,
//! `.class`/`#id` shortcuts, `{…}` Ruby-hash attributes (via the runtime
//! `render_attrs` helper), `=`/`-` output & silent code (incl. `… do |x|`
//! blocks and `if/else` middle markers), `#{}` text interpolation, the
//! `:ruby` filter, doctypes, and `-#` / `/` comments. The fiddly tail
//! that Mastodon does not use (`(…)` html attrs, `~`, multiline `|`,
//! object refs, non-`:ruby` filters) is recorded as a survey gap rather
//! than mis-compiled. Whitespace-removal `<`/`>` is parsed but not yet
//! semantically applied (a byte-identical conformance follow-on).

use crate::erb::{ErbSegment, is_block_expr, opens_passthrough_block, ruby_string_literal};

/// HTML void elements: rendered without a close tag and never opening a
/// child frame.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Ruby control-flow continuations: at the same indent as their opener,
/// they continue the block (emit inline, keep the frame) rather than
/// closing it.
fn is_middle_marker(code: &str) -> bool {
    let head = code.trim_start();
    for kw in ["else", "elsif", "when", "in", "rescue", "ensure"] {
        if head == kw || head.starts_with(&format!("{kw} ")) {
            return true;
        }
    }
    false
}

/// How an open frame closes when the indentation drops below it.
enum Close {
    /// An HTML element — emit `</tag>` as buffer text.
    Tag(String),
    /// A Ruby control block (`- if`, `- each do`, …) — emit `end`.
    RubyEnd,
    /// An output block (`= form_with … do |f|`) — emit `end).to_s`.
    OutputEnd,
    /// Arbitrary trailing text (HTML-comment `-->`).
    Text(String),
    /// Nothing (`-#` skip, `:ruby` / unsupported-filter capture).
    Nothing,
}

/// How child lines (more indented than this frame) are handled.
#[derive(PartialEq)]
enum Capture {
    /// Normal HAML nesting.
    None,
    /// `:ruby` filter — children are raw Ruby statements.
    RawRuby,
    /// `-#` comment / unsupported filter — children are dropped.
    Skip,
}

struct Frame {
    indent: usize,
    close: Close,
    capture: Capture,
    /// Ruby control block — eligible to be continued by an `else`/`when`/…
    /// middle marker at the same indent.
    ruby_block: bool,
}

/// Compile HAML source to the `_buf`-append Ruby program.
pub fn compile_haml(source: &str) -> String {
    compile_haml_mapped(source).0
}

/// As [`compile_haml`], plus the compiled-Ruby ↔ template segment map
/// (see [`ErbSegment`]) the view pipeline uses to translate spans back to
/// template coordinates.
pub fn compile_haml_mapped(source: &str) -> (String, Vec<ErbSegment>) {
    let mut c = Compiler {
        out: String::new(),
        map: Vec::new(),
        stack: Vec::new(),
    };
    c.out.push_str("_buf = \"\"\n");

    let lines = split_lines(source);
    let mut i = 0;
    while i < lines.len() {
        let (line_start, line) = lines[i];
        i += 1;
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();
        if content.is_empty() {
            continue;
        }

        // Inside a capture frame (`:ruby` / `-#`), deeper lines are taken
        // verbatim (or dropped) without HAML parsing.
        if let Some(top) = c.stack.last() {
            if top.capture != Capture::None && indent > top.indent {
                if top.capture == Capture::RawRuby {
                    let e_start = line_start + indent;
                    c.code(content, e_start);
                }
                continue;
            }
        }

        // Pull in trailing-comma Ruby continuations (e.g.
        // `= react_component :video,` or `%p= t key,` spanning several
        // lines). The joined code maps to the first line's range (spans
        // clamp).
        let mut content = content.to_string();
        if ruby_tail_can_continue(&content) {
            while content.trim_end().ends_with(',') && i < lines.len() {
                let (_, cont_line) = lines[i];
                i += 1;
                content.push(' ');
                content.push_str(cont_line.trim_start());
            }
        }

        let middle = content.starts_with('-')
            && is_middle_marker(content.trim_start_matches('-').trim_start());
        c.close_to(indent, middle);
        c.line(&content, indent, line_start);
    }

    while let Some(f) = c.stack.pop() {
        c.emit_close(&f);
    }
    c.out.push_str("_buf\n");
    (c.out, c.map)
}

struct Compiler {
    out: String,
    map: Vec<ErbSegment>,
    stack: Vec<Frame>,
}

impl Compiler {
    /// Append a static buffer text chunk (no segment — synthesized glue).
    fn text(&mut self, s: &str) {
        self.out.push_str("_buf = _buf + ");
        self.out.push_str(&ruby_string_literal(s));
        self.out.push('\n');
    }

    /// Append a template-derived buffer text chunk, mapped back to its
    /// source range so text diagnostics attribute correctly.
    fn text_mapped(&mut self, s: &str, e_start: usize, e_end: usize) {
        self.out.push_str("_buf = _buf + ");
        let c_start = self.out.len();
        self.out.push_str(&ruby_string_literal(s));
        self.seg(c_start, e_start, e_end);
        self.out.push('\n');
    }

    /// Emit raw Ruby code (a `- code` line / `:ruby` body line), mapped to
    /// its template range.
    fn code(&mut self, code: &str, e_start: usize) {
        let c_start = self.out.len();
        self.out.push_str(code);
        self.seg(c_start, e_start, e_start + code.len());
        self.out.push('\n');
    }

    fn seg(&mut self, c_start: usize, e_start: usize, e_end: usize) {
        self.map.push(ErbSegment {
            c_start: c_start as u32,
            c_end: self.out.len() as u32,
            e_start: e_start as u32,
            e_end: e_end as u32,
        });
    }

    /// Close every frame at or below `indent`. A middle marker
    /// (`else`/`when`/…) at the same indent as a Ruby block keeps that
    /// block open so the marker continues it.
    fn close_to(&mut self, indent: usize, middle: bool) {
        while let Some(top) = self.stack.last() {
            if top.indent < indent {
                break;
            }
            if middle && top.indent == indent && top.ruby_block {
                break;
            }
            let f = self.stack.pop().unwrap();
            self.emit_close(&f);
        }
    }

    fn emit_close(&mut self, f: &Frame) {
        match &f.close {
            Close::Tag(tag) => self.text(&format!("</{tag}>")),
            Close::RubyEnd => self.out.push_str("end\n"),
            Close::OutputEnd => self.out.push_str("end).to_s\n"),
            Close::Text(t) => self.text(t),
            Close::Nothing => {}
        }
    }

    /// Compile one (already continuation-joined) HAML line.
    fn line(&mut self, content: &str, indent: usize, line_start: usize) {
        let e_base = line_start + indent;
        let first = content.as_bytes()[0];

        // `-#` HAML comment — drop the line and any nested block.
        if content.starts_with("-#") {
            self.stack.push(Frame {
                indent,
                close: Close::Nothing,
                capture: Capture::Skip,
                ruby_block: false,
            });
            return;
        }

        match first {
            b'!' if content.starts_with("!!!") => {
                // Doctype. The subset only needs HTML5; the bare `!!!`
                // (XHTML) is approximated to it (rare, layouts only).
                self.text("<!DOCTYPE html>\n");
            }
            b'/' => self.html_comment(content[1..].trim(), indent),
            b':' => self.filter(content[1..].trim(), indent),
            b'=' => self.output(content[1..].trim(), false, indent, e_base + 1),
            b'~' => self.output(content[1..].trim(), false, indent, e_base + 1),
            b'&' if content.starts_with("&=") => {
                self.output(content[2..].trim(), false, indent, e_base + 2)
            }
            b'!' if content.starts_with("!=") => {
                // Raw (unescaped) output. v1 emits the same `.to_s` form;
                // the escape distinction is a conformance follow-on.
                self.output(content[2..].trim(), true, indent, e_base + 2)
            }
            b'-' => self.silent(content[1..].trim_start(), indent, e_base + 1),
            b'%' => self.element(content, indent, e_base),
            // `.`/`#` start an implicit-div shortcut only when followed by a
            // name char; otherwise the line is text — notably `#{expr}`
            // interpolation, which must NOT be read as an `#id`.
            b'.' | b'#' if content.as_bytes().get(1).is_some_and(|&b| is_name_char(b)) => {
                self.element(content, indent, e_base)
            }
            b'\\' => {
                // `\` escapes the first char so a literal line can begin
                // with `%`/`=`/etc.
                self.text_mapped(content, e_base + 1, e_base + content.len());
            }
            _ => self.text_mapped(content, e_base, e_base + content.len()),
        }
    }

    /// `= expr` / `~ expr` / `!= expr` — buffer-output an expression,
    /// opening an output block when it ends in `do`/`{`.
    fn output(&mut self, expr: &str, _raw: bool, indent: usize, e_start: usize) {
        self.out.push_str("_buf = _buf + (");
        let c_start = self.out.len();
        self.out.push_str(expr);
        self.seg(c_start, e_start, e_start + expr.len());
        if is_block_expr(expr) {
            // `= form_with … do |f|` — same structural shape ERB produces
            // (`(EXPR do …  end).to_s`), so the view lowerer's block-yielder
            // handling treats it identically. Closes on dedent.
            self.out.push('\n');
            self.stack.push(Frame {
                indent,
                close: Close::OutputEnd,
                capture: Capture::None,
                ruby_block: false,
            });
        } else {
            // Close on a new line so a trailing Ruby comment in the
            // expression (`= f.hidden_field :id if x # note`) can't comment
            // out the `).to_s`.
            self.out.push_str("\n).to_s\n");
        }
    }

    /// `- code` — passthrough Ruby. Opens a `end`-closed frame when it
    /// starts a block; middle markers (`else`/…) emit inline and continue
    /// the frame `close_to` kept open.
    fn silent(&mut self, code: &str, indent: usize, e_start: usize) {
        self.code(code, e_start);
        if is_middle_marker(code) {
            return;
        }
        if opens_passthrough_block(code) {
            self.stack.push(Frame {
                indent,
                close: Close::RubyEnd,
                capture: Capture::None,
                ruby_block: true,
            });
        }
    }

    fn filter(&mut self, name: &str, indent: usize) {
        let capture = if name == "ruby" {
            Capture::RawRuby
        } else {
            // `:javascript` / `:css` / … aren't in the Mastodon subset.
            crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                file: String::new(),
                message: format!("haml filter not supported: :{name}"),
            });
            Capture::Skip
        };
        self.stack.push(Frame {
            indent,
            close: Close::Nothing,
            capture,
            ruby_block: false,
        });
    }

    fn html_comment(&mut self, rest: &str, indent: usize) {
        self.text("<!--");
        if !rest.is_empty() {
            self.text(&format!(" {rest}"));
        }
        self.stack.push(Frame {
            indent,
            close: Close::Text(" -->".to_string()),
            capture: Capture::None,
            ruby_block: false,
        });
    }

    /// `%tag.class#id{attrs}/ content` and `.class`/`#id` implicit divs.
    fn element(&mut self, content: &str, indent: usize, e_base: usize) {
        let head = parse_element_head(content);
        let tag = head.tag;

        // Open tag: static text when there's no `{…}` hash, else route the
        // merged attributes through the runtime `render_attrs` helper.
        match &head.attrs {
            None => {
                let mut open = format!("<{tag}");
                if !head.classes.is_empty() {
                    open.push_str(&format!(" class=\"{}\"", head.classes.join(" ")));
                }
                if let Some(id) = &head.id {
                    open.push_str(&format!(" id=\"{id}\""));
                }
                open.push('>');
                self.text(&open);
            }
            Some(body) => {
                self.text(&format!("<{tag}"));
                // Folded shortcut entries (`.class`/`#id`), skipping a key
                // the hash already sets.
                let mut shortcuts = String::new();
                if !head.classes.is_empty() {
                    if body.contains("class:") {
                        // shortcut class + hash `class:` merge isn't modeled.
                        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                            file: String::new(),
                            message: "haml class shortcut + hash class: merge not supported".into(),
                        });
                    } else {
                        shortcuts.push_str(&format!("class: {:?}, ", head.classes.join(" ")));
                    }
                }
                if let Some(id) = &head.id {
                    if !body.contains("id:") {
                        shortcuts.push_str(&format!("id: {id:?}, "));
                    }
                }
                // The `{…}` body is either hash-literal contents (`a: 1, b: 2`)
                // or a bare hash expression (`%tag{ html_attributes }`); the
                // latter must NOT be wrapped in `{ }`.
                let hash_expr = if looks_like_hash_pairs(body) {
                    format!("{{ {shortcuts}{body} }}")
                } else if shortcuts.is_empty() {
                    body.clone()
                } else {
                    format!("{{ {} }}.merge({body})", shortcuts.trim_end().trim_end_matches(','))
                };
                self.out.push_str("_buf = _buf + (render_attrs(");
                let c_start = self.out.len();
                self.out.push_str(&hash_expr);
                // Map the attribute code back to the `{…}` body's range.
                self.seg(c_start, head.attrs_range.0 + e_base, head.attrs_range.1 + e_base);
                self.out.push_str(")).to_s\n");
                self.text(">");
            }
        }

        let void = head.self_close || VOID_ELEMENTS.contains(&tag.as_str());
        if void {
            return;
        }

        let rest = head.rest.trim_start();
        if rest.is_empty() {
            // Block element — children render, dedent emits `</tag>`.
            self.stack.push(Frame {
                indent,
                close: Close::Tag(tag),
                capture: Capture::None,
                ruby_block: false,
            });
        } else if let Some(expr) = rest.strip_prefix("!=") {
            self.inline_output(expr.trim(), &tag);
        } else if let Some(expr) = rest.strip_prefix('=') {
            self.inline_output(expr.trim(), &tag);
        } else {
            // Inline text content.
            let off = e_base + (content.len() - rest.len());
            self.out.push_str("_buf = _buf + ");
            let c_start = self.out.len();
            self.out.push_str(&ruby_string_literal(rest));
            self.seg(c_start, off, off + rest.len());
            self.out.push('\n');
            self.text(&format!("</{tag}>"));
        }
    }

    /// `%tag= expr` — emit `<tag>` + expr + `</tag>` inline (no frame).
    fn inline_output(&mut self, expr: &str, tag: &str) {
        self.out.push_str("_buf = _buf + (");
        self.out.push_str(expr);
        // Newline-isolate the closer (trailing-comment safety, as `output`).
        self.out.push_str("\n).to_s\n");
        self.text(&format!("</{tag}>"));
    }
}

/// Does the `{…}` attribute body look like hash-literal contents
/// (`key: val` / `'k' => v`, comma-separated pairs) rather than a single
/// bare hash expression (`html_attributes`)? Scans at brace/paren/bracket
/// depth 0, skipping string contents.
fn looks_like_hash_pairs(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => return true,
            b'=' if depth == 0 && bytes.get(i + 1) == Some(&b'>') => return true,
            b':' if depth == 0 => {
                // A `key:` pair separator — not `::` and not a `:symbol`.
                let prev = if i > 0 { bytes[i - 1] } else { b' ' };
                let next = bytes.get(i + 1).copied().unwrap_or(b' ');
                if prev != b':' && next == b' ' {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Does this line's content carry a Ruby tail that a trailing comma could
/// continue onto the next line? True for output/silent lines and for
/// elements with inline `=` output; false for plain text (so `%p Hello,`
/// is not mis-joined).
fn ruby_tail_can_continue(content: &str) -> bool {
    match content.as_bytes().first() {
        Some(b'=') | Some(b'~') => true,
        Some(b'-') => !content.starts_with("-#"),
        Some(b'&') => content.starts_with("&="),
        Some(b'!') => content.starts_with("!="),
        Some(b'%') | Some(b'.') | Some(b'#') => content.contains("= "),
        _ => false,
    }
}

struct ElementHead {
    tag: String,
    classes: Vec<String>,
    id: Option<String>,
    /// The `{…}` attribute-hash body (without braces), if present.
    attrs: Option<String>,
    /// Byte range of `attrs` within the *line content* (for span mapping).
    attrs_range: (usize, usize),
    self_close: bool,
    /// Everything after the tag head (inline content / output).
    rest: String,
}

/// Parse the `%tag.class#id{attrs}/` head of an element line.
fn parse_element_head(content: &str) -> ElementHead {
    let bytes = content.as_bytes();
    let mut pos = 0;
    let mut tag = String::new();

    if bytes[pos] == b'%' {
        pos += 1;
        while pos < bytes.len() && is_tag_char(bytes[pos]) {
            tag.push(bytes[pos] as char);
            pos += 1;
        }
    }
    if tag.is_empty() {
        tag.push_str("div");
    }

    let mut classes = Vec::new();
    let mut id = None;
    while pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b'#') {
        let kind = bytes[pos];
        pos += 1;
        let start = pos;
        while pos < bytes.len() && is_name_char(bytes[pos]) {
            pos += 1;
        }
        let name = &content[start..pos];
        if kind == b'.' {
            classes.push(name.to_string());
        } else {
            id = Some(name.to_string());
        }
    }

    let mut attrs = None;
    let mut attrs_range = (0, 0);
    if pos < bytes.len() && bytes[pos] == b'{' {
        if let Some(end) = matching_brace(bytes, pos) {
            attrs = Some(content[pos + 1..end].trim().to_string());
            attrs_range = (pos + 1, end);
            pos = end + 1;
        }
    }

    // Consume trailing markers: `/` self-close, `<`/`>` whitespace removal.
    let mut self_close = false;
    while pos < bytes.len() {
        match bytes[pos] {
            b'/' => {
                self_close = true;
                pos += 1;
            }
            b'<' | b'>' => pos += 1, // whitespace removal — parsed, not yet applied
            _ => break,
        }
    }

    ElementHead {
        tag,
        classes,
        id,
        attrs,
        attrs_range,
        self_close,
        rest: content[pos..].to_string(),
    }
}

fn is_tag_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Index of the `}` matching the `{` at `open`, ignoring nesting depth.
/// (Does not track string contents — adequate for the subset's hashes.)
fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split into `(byte_offset, line_without_newline)` pairs.
fn split_lines(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            out.push((start, &source[start..i]));
            start = i + 1;
        }
    }
    if start < source.len() {
        out.push((start, &source[start..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_tag_and_class_shortcut() {
        let (ruby, _) = compile_haml_mapped("%p hello\n.box\n");
        assert!(ruby.contains("_buf = _buf + \"<p>\""), "got:\n{ruby}");
        assert!(ruby.contains("_buf = _buf + \"hello\""));
        assert!(ruby.contains("_buf = _buf + \"</p>\""));
        assert!(ruby.contains("_buf = _buf + \"<div class=\\\"box\\\">\""));
        assert!(ruby.contains("_buf = _buf + \"</div>\""));
    }

    #[test]
    fn inline_output_and_id() {
        let (ruby, _) = compile_haml_mapped("%h1#title= @post.name\n");
        assert!(ruby.contains("<h1 id=\\\"title\\\">"), "got:\n{ruby}");
        assert!(ruby.contains("_buf = _buf + (@post.name\n).to_s"), "got:\n{ruby}");
        assert!(ruby.contains("</h1>"));
    }

    #[test]
    fn silent_block_closes_with_end() {
        let (ruby, _) = compile_haml_mapped("- if x\n  %p yes\n");
        assert!(ruby.contains("if x\n"), "got:\n{ruby}");
        assert!(ruby.contains("<p>"));
        assert!(ruby.trim_end().contains("end\n"), "block must close: {ruby}");
    }

    #[test]
    fn if_else_middle_marker_keeps_block_open() {
        let (ruby, _) = compile_haml_mapped("- if x\n  a\n- else\n  b\n");
        // exactly one `end`, with `else` continuing the block.
        assert_eq!(ruby.matches("\nend\n").count(), 1, "got:\n{ruby}");
        assert!(ruby.contains("else\n"), "got:\n{ruby}");
    }

    #[test]
    fn output_do_block_closes_with_end_to_s() {
        let (ruby, _) = compile_haml_mapped("= form_with model: @p do |f|\n  %p inner\n");
        assert!(ruby.contains("_buf = _buf + (form_with model: @p do |f|"), "got:\n{ruby}");
        assert!(ruby.contains("end).to_s"), "got:\n{ruby}");
    }

    #[test]
    fn attribute_hash_via_render_attrs() {
        let (ruby, _) = compile_haml_mapped("%a{href: url, data: { id: x }} link\n");
        assert!(ruby.contains("render_attrs({ href: url, data: { id: x } })"), "got:\n{ruby}");
        assert!(ruby.contains("_buf = _buf + \"<a\""));
    }

    #[test]
    fn class_shortcut_folds_into_attr_hash() {
        let (ruby, _) = compile_haml_mapped("%span.flag{title: t}\n");
        assert!(ruby.contains("render_attrs({ class: \"flag\", title: t })"), "got:\n{ruby}");
    }

    #[test]
    fn void_and_self_close_have_no_close_tag() {
        let (ruby, _) = compile_haml_mapped("%hr.spacer/\n%br\n%meta{name: x}\n");
        assert!(!ruby.contains("</hr>"), "got:\n{ruby}");
        assert!(!ruby.contains("</br>"));
        assert!(!ruby.contains("</meta>"));
    }

    #[test]
    fn ruby_filter_emits_body_as_code() {
        let (ruby, _) = compile_haml_mapped(":ruby\n  primary.item :x, y\n");
        assert!(ruby.contains("primary.item :x, y\n"), "got:\n{ruby}");
        assert!(!ruby.contains("\"primary"), "filter body must be code, not text: {ruby}");
    }

    #[test]
    fn comment_is_dropped() {
        let (ruby, _) = compile_haml_mapped("-# hidden\n%p shown\n");
        assert!(!ruby.contains("hidden"), "got:\n{ruby}");
        assert!(ruby.contains("<p>"));
    }

    #[test]
    fn nesting_closes_on_dedent() {
        let (ruby, _) = compile_haml_mapped(".outer\n  .inner\n    = x\n");
        // Both divs open and both close, in order.
        let opens = ruby.matches("<div class=").count();
        let closes = ruby.matches("</div>").count();
        assert_eq!(opens, 2, "got:\n{ruby}");
        assert_eq!(closes, 2, "got:\n{ruby}");
    }

    // --- regressions found against real Mastodon HAML ---

    #[test]
    fn hash_interpolation_line_is_text_not_id() {
        // `#{expr}` at line start is interpolated text, not an `#id` div.
        let (ruby, _) = compile_haml_mapped("#{n + 1}.\n");
        assert!(ruby.contains("_buf = _buf + \"#{n + 1}.\""), "got:\n{ruby}");
        assert!(!ruby.contains("id="), "got:\n{ruby}");
    }

    #[test]
    fn trailing_ruby_comment_does_not_break_closer() {
        let (ruby, _) = compile_haml_mapped("= f.hidden_field :id if x # note\n");
        assert!(ruby.contains("# note\n).to_s"), "got:\n{ruby}");
    }

    #[test]
    fn bare_hash_expression_attrs_pass_through() {
        // `%tag{ a_hash_method }` — the body is the hash itself, not pairs.
        let (ruby, _) = compile_haml_mapped("%html{ html_attributes }\n");
        assert!(ruby.contains("render_attrs(html_attributes)"), "got:\n{ruby}");
    }

    #[test]
    fn inline_output_multiline_continuation_joins() {
        let (ruby, _) = compile_haml_mapped("%p= t key,\n  foo: 1\n");
        assert!(ruby.contains("t key, foo: 1"), "got:\n{ruby}");
    }

    #[test]
    fn program_has_buf_prologue_and_epilogue() {
        let (ruby, _) = compile_haml_mapped("%p hi\n");
        assert!(ruby.starts_with("_buf = \"\"\n"));
        assert_eq!(ruby.trim_end().lines().last(), Some("_buf"));
    }
}
