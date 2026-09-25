//! Builder (`.builder`) template compiler.
//!
//! Like [`crate::erb`] and [`crate::haml`], this compiles a template to
//! the `_buf`-append Ruby shape (`_buf = ""` … `_buf = _buf + EXPR` …
//! `_buf`) plus a segment map, so the result flows through the shared
//! `ingest_template` view pipeline unchanged.
//!
//! A Builder template is Ruby: Rails' handler runs it with
//! `xml = ::Builder::XmlMarkup.new(indent: 2, target: output_buffer)`
//! (actionview `template/handlers/builder.rb`), and every `xml.<name>`
//! call appends an element through `method_missing`. What makes it
//! compilable is that the element NESTING is lexical — Builder's indent
//! level is the depth of `xml.foo do … end` blocks — so the markup
//! around each element is known here, and only the text and attribute
//! VALUES are runtime expressions.
//!
//! Output matches builder 3.3's `XmlMarkup`:
//!
//! - `xml.name "text", attr: v` → `<name attr="v">text</name>`, a
//!   leading Symbol argument naming a namespace (`xml.dc :creator` →
//!   `<dc:creator>`); `xml.tag!("atom:link", …)` names the tag directly.
//! - With a block: the open tag, the block's elements one level deeper,
//!   the close tag. With neither text nor block: `<name attr="v"/>`.
//! - Two spaces of indent per level and a newline after each element.
//! - `xml.instruct!` → `<?xml version="1.0" encoding="UTF-8"?>`.
//! - Text escapes `&`, `<`, `>` (`_escape`); attribute values also
//!   `"`, newline and carriage return (`_escape_attribute`); a Symbol
//!   attribute value is written as-is.
//!
//! Runtime values are escaped by `ActionView::ViewHelpers.builder_text`
//! / `builder_attr` (runtime/ruby/action_view/view_helpers.rb) and
//! marked `html_safe` so the view walker's HTML auto-escape does not
//! escape them a second time — Builder's rules are not HTML's (a `"` in
//! text stays literal).
//!
//! Any other statement is copied through as Ruby, recursing into the
//! blocks and conditionals that contain `xml` calls. A shape this does
//! not model (`xml.cdata!`, a dynamic `tag!` name, text plus a block) is
//! recorded as a gap and compiled to nothing rather than guessed at.

use ruby_prism::Node;

use crate::erb::{ErbSegment, ruby_string_literal};

/// Compile Builder template source to the `_buf`-append Ruby program.
pub fn compile_builder(source: &str) -> String {
    compile_builder_mapped(source).0
}

/// [`compile_builder`] plus the compiled↔template segment map.
pub fn compile_builder_mapped(source: &str) -> (String, Vec<ErbSegment>) {
    let result = ruby_prism::parse(source.as_bytes());
    let mut c = Compiler { src: source, out: String::new(), map: Vec::new(), pending: String::new() };
    c.out.push_str("_buf = \"\"\n");
    let root = result.node();
    if let Some(program) = root.as_program_node() {
        c.statements(&program.statements().as_node(), 0);
    }
    c.flush();
    c.out.push_str("_buf\n");
    (c.out, c.map)
}

struct Compiler<'s> {
    src: &'s str,
    out: String,
    map: Vec<ErbSegment>,
    /// Static markup not yet written: adjacent pieces (`<?xml`, each
    /// attribute, `?>`) join into ONE buffer append, so the rendered
    /// view does one `<<` per run of static text rather than one per
    /// fragment.
    pending: String,
}

impl Compiler<'_> {
    fn slice(&self, loc: &ruby_prism::Location<'_>) -> &str {
        &self.src[loc.start_offset()..loc.end_offset()]
    }

    fn text(&mut self, s: &str) {
        self.pending.push_str(s);
    }

    /// Write the pending static text. Called before anything that is
    /// not static text reaches the output.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let s = std::mem::take(&mut self.pending);
        self.out.push_str("_buf = _buf + ");
        self.out.push_str(&ruby_string_literal(&s));
        self.out.push('\n');
    }

    /// Copy a template code range through verbatim, mapped back to it.
    fn code(&mut self, start: usize, end: usize) {
        self.flush();
        let c_start = self.out.len();
        self.out.push_str(&self.src[start..end]);
        self.map.push(ErbSegment {
            c_start: c_start as u32,
            c_end: self.out.len() as u32,
            e_start: start as u32,
            e_end: end as u32,
        });
        self.out.push('\n');
    }

    /// Append a runtime value through one of the Builder escapes.
    fn escaped(&mut self, helper: &str, loc: &ruby_prism::Location<'_>) {
        self.flush();
        self.out.push_str("_buf = _buf + (ActionView::ViewHelpers.");
        self.out.push_str(helper);
        self.out.push_str("((");
        let (start, end) = (loc.start_offset(), loc.end_offset());
        let c_start = self.out.len();
        self.out.push_str(&self.src[start..end]);
        self.map.push(ErbSegment {
            c_start: c_start as u32,
            c_end: self.out.len() as u32,
            e_start: start as u32,
            e_end: end as u32,
        });
        self.out.push_str(").to_s).html_safe).to_s\n");
    }

    /// Write generated Ruby (control flow, append glue) after any
    /// pending static text.
    fn raw(&mut self, s: &str) {
        self.flush();
        self.out.push_str(s);
    }

    fn gap(&mut self, what: &str) {
        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
            file: String::new(),
            message: format!("builder template: {what} is not compiled"),
        });
    }

    fn statements(&mut self, node: &Node<'_>, level: usize) {
        if let Some(stmts) = node.as_statements_node() {
            for s in stmts.body().iter() {
                self.statement(&s, level);
            }
        } else {
            self.statement(node, level);
        }
    }

    fn statement(&mut self, node: &Node<'_>, level: usize) {
        if let Some(call) = node.as_call_node() {
            if is_xml_receiver(&call) {
                self.xml_call(&call, level);
                return;
            }
            // A block that holds elements: `@stories.each do |story|`.
            if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                if contains_xml(&block.as_node()) {
                    let start = call.location().start_offset();
                    let mut head_end = block.opening_loc().start_offset();
                    while head_end > start && self.src.as_bytes()[head_end - 1].is_ascii_whitespace() {
                        head_end -= 1;
                    }
                    self.code_inline(start, head_end);
                    self.raw(" do");
                    if let Some(params) = block.parameters() {
                        self.raw(" ");
                        let loc = params.location();
                        let text = self.slice(&loc).to_string();
                        self.raw(&text);
                    }
                    self.raw("\n");
                    if let Some(body) = block.body() {
                        self.statements(&body, level);
                    }
                    self.raw("end\n");
                    return;
                }
            }
        }
        if let Some(if_node) = node.as_if_node() {
            if contains_xml(node) {
                self.if_chain(&if_node, level, "if");
                return;
            }
        }
        if let Some(unless) = node.as_unless_node() {
            if contains_xml(node) {
                self.raw("unless ");
                let pred = unless.predicate().location();
                self.code_inline(pred.start_offset(), pred.end_offset());
                self.raw("\n");
                if let Some(s) = unless.statements() {
                    self.statements(&s.as_node(), level);
                }
                if let Some(e) = unless.else_clause() {
                    self.raw("else\n");
                    if let Some(s) = e.statements() {
                        self.statements(&s.as_node(), level);
                    }
                }
                self.raw("end\n");
                return;
            }
        }
        let loc = node.location();
        self.code(loc.start_offset(), loc.end_offset());
    }

    /// Copy a code range with no trailing newline.
    fn code_inline(&mut self, start: usize, end: usize) {
        self.flush();
        let c_start = self.out.len();
        self.raw(&self.src[start..end]);
        self.map.push(ErbSegment {
            c_start: c_start as u32,
            c_end: self.out.len() as u32,
            e_start: start as u32,
            e_end: end as u32,
        });
    }

    /// `if` / `elsif` chains and the modifier form (`xml.x y if cond`),
    /// which prism represents the same way.
    fn if_chain(&mut self, node: &ruby_prism::IfNode<'_>, level: usize, keyword: &str) {
        self.raw(keyword);
        self.raw(" ");
        let pred = node.predicate().location();
        self.code_inline(pred.start_offset(), pred.end_offset());
        self.raw("\n");
        if let Some(s) = node.statements() {
            self.statements(&s.as_node(), level);
        }
        match node.subsequent() {
            Some(sub) => {
                if let Some(elsif) = sub.as_if_node() {
                    self.if_chain(&elsif, level, "elsif");
                    return;
                }
                if let Some(e) = sub.as_else_node() {
                    self.raw("else\n");
                    if let Some(s) = e.statements() {
                        self.statements(&s.as_node(), level);
                    }
                }
                self.raw("end\n");
            }
            None => self.raw("end\n"),
        }
    }

    fn xml_call(&mut self, call: &ruby_prism::CallNode<'_>, level: usize) {
        let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
        let args: Vec<Node<'_>> =
            call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let indent = " ".repeat(level * 2);
        match name.as_str() {
            "instruct!" => {
                let directive = args
                    .first()
                    .and_then(|a| a.as_symbol_node())
                    .map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
                    .unwrap_or_else(|| "xml".to_string());
                let mut attrs: Vec<(String, AttrValue)> = Vec::new();
                if directive == "xml" {
                    attrs.push(("version".into(), AttrValue::Static("1.0".into())));
                    attrs.push(("encoding".into(), AttrValue::Static("UTF-8".into())));
                }
                for a in &args {
                    self.collect_attrs(a, &mut attrs);
                }
                // Builder writes `version`, `encoding`, `standalone` first.
                let order = ["version", "encoding", "standalone"];
                let mut sorted: Vec<(String, AttrValue)> = Vec::new();
                for k in order {
                    if let Some(i) = attrs.iter().position(|(n, _)| n == k) {
                        sorted.push(attrs.remove(i));
                    }
                }
                sorted.extend(attrs);
                self.text(&format!("{indent}<?{directive}"));
                self.attrs(&sorted);
                self.text("?>\n");
            }
            "<<" => {
                // Raw text, unescaped: `xml << "<br/>"`.
                if let Some(a) = args.first() {
                    let loc = a.location();
                    self.raw("_buf = _buf + ((");
                    self.code_inline(loc.start_offset(), loc.end_offset());
                    self.raw(").to_s.html_safe).to_s\n");
                }
            }
            "text!" => {
                if let Some(a) = args.first() {
                    self.text_value(a);
                }
            }
            "cdata!" | "cdata_value!" | "comment!" | "declare!" | "target!" => {
                self.gap(&format!("`xml.{name}`"));
            }
            _ => {
                let (mut tag, rest) = if name == "tag!" {
                    match args.first().and_then(static_name) {
                        Some(n) => (n, &args[1..]),
                        None => {
                            self.gap("a `tag!` with a non-literal name");
                            return;
                        }
                    }
                } else {
                    (name.clone(), &args[..])
                };
                let mut rest: &[Node<'_>] = rest;
                // `xml.dc :creator` — Builder reads a leading Symbol as
                // the local name under the method's namespace.
                if let Some(ns_local) = rest.first().and_then(|a| a.as_symbol_node()) {
                    tag = format!("{tag}:{}", String::from_utf8_lossy(ns_local.unescaped()));
                    rest = &rest[1..];
                }
                let mut attrs: Vec<(String, AttrValue)> = Vec::new();
                let mut texts: Vec<&Node<'_>> = Vec::new();
                for a in rest {
                    if a.as_keyword_hash_node().is_some() || a.as_hash_node().is_some() {
                        self.collect_attrs(a, &mut attrs);
                    } else if a.as_nil_node().is_some() {
                        // Builder ignores a nil argument (no explicit-nil mode).
                    } else {
                        texts.push(a);
                    }
                }
                let block = call.block().and_then(|b| b.as_block_node());
                if block.is_some() && !texts.is_empty() {
                    self.gap("an element with both text and a block");
                    return;
                }
                self.text(&format!("{indent}<{tag}"));
                self.attrs(&attrs);
                if let Some(block) = block {
                    self.text(">\n");
                    if let Some(body) = block.body() {
                        self.statements(&body, level + 1);
                    }
                    self.text(&format!("{indent}</{tag}>\n"));
                } else if texts.is_empty() {
                    self.text("/>\n");
                } else {
                    self.text(">");
                    for t in texts {
                        self.text_value(t);
                    }
                    self.text(&format!("</{tag}>\n"));
                }
            }
        }
    }

    /// A text argument: a literal escaped here, anything else through
    /// `builder_text` at run time.
    fn text_value(&mut self, node: &Node<'_>) {
        match plain_string(node) {
            Some(s) => self.text(&escape_text(&s)),
            None => self.escaped("builder_text", &node.location()),
        }
    }

    fn collect_attrs(&self, node: &Node<'_>, out: &mut Vec<(String, AttrValue)>) {
        let elements: Vec<Node<'_>> = if let Some(h) = node.as_keyword_hash_node() {
            h.elements().iter().collect()
        } else if let Some(h) = node.as_hash_node() {
            h.elements().iter().collect()
        } else {
            return;
        };
        for el in elements {
            let Some(pair) = el.as_assoc_node() else { continue };
            let Some(key) = static_name(&pair.key()) else { continue };
            let value = pair.value();
            let v = if let Some(sym) = value.as_symbol_node() {
                AttrValue::Static(String::from_utf8_lossy(sym.unescaped()).into_owned())
            } else if let Some(s) = plain_string(&value) {
                AttrValue::Static(escape_attr(&s))
            } else {
                let loc = value.location();
                AttrValue::Dynamic(loc.start_offset(), loc.end_offset())
            };
            if let Some(i) = out.iter().position(|(k, _)| k == &key) {
                out[i].1 = v;
            } else {
                out.push((key, v));
            }
        }
    }

    fn attrs(&mut self, attrs: &[(String, AttrValue)]) {
        for (k, v) in attrs {
            match v {
                AttrValue::Static(s) => self.text(&format!(" {k}=\"{s}\"")),
                AttrValue::Dynamic(start, end) => {
                    self.text(&format!(" {k}=\""));
                    self.raw("_buf = _buf + (ActionView::ViewHelpers.builder_attr((");
                    self.code_inline(*start, *end);
                    self.raw(").to_s).html_safe).to_s\n");
                    self.text("\"");
                }
            }
        }
    }
}

enum AttrValue {
    /// Already in its final (escaped) form.
    Static(String),
    /// A template range whose value is escaped at run time.
    Dynamic(usize, usize),
}

/// `xml` as a call receiver — the template's builder object. Prism reads
/// the bare name as a receiverless call (the handler defines the local,
/// the template never does).
fn is_xml_receiver(call: &ruby_prism::CallNode<'_>) -> bool {
    let Some(recv) = call.receiver() else { return false };
    if let Some(r) = recv.as_call_node() {
        return r.receiver().is_none()
            && r.arguments().is_none()
            && r.name().as_slice() == b"xml";
    }
    if let Some(v) = recv.as_local_variable_read_node() {
        return v.name().as_slice() == b"xml";
    }
    false
}

/// Does any `xml.<…>` call sit anywhere under this node?
fn contains_xml(node: &Node<'_>) -> bool {
    struct V {
        found: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for V {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if is_xml_receiver(node) {
                self.found = true;
                return;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut v = V { found: false };
    ruby_prism::Visit::visit(&mut v, node);
    v.found
}

/// A String or Symbol literal's text (a tag or attribute NAME).
fn static_name(node: &Node<'_>) -> Option<String> {
    if let Some(s) = node.as_symbol_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    plain_string(node)
}

/// A String literal without interpolation.
fn plain_string(node: &Node<'_>) -> Option<String> {
    node.as_string_node()
        .map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
}

/// Builder's `_escape`: `&`, `<`, `>`.
pub fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Builder's `_escape_attribute`: the text escape plus `"`, `\n`, `\r`.
pub fn escape_attr(s: &str) -> String {
    escape_text(s).replace('\n', "&#10;").replace('\r', "&#13;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(compiled: &str) -> String {
        // The static parts, concatenated — enough to check the markup.
        compiled
            .lines()
            .filter_map(|l| l.strip_prefix("_buf = _buf + \""))
            .filter_map(|l| l.strip_suffix('"'))
            .map(|l| l.replace("\\n", "\n").replace("\\\"", "\""))
            .collect()
    }

    #[test]
    fn nesting_indents_two_per_level_and_self_closes() {
        let out = compile_builder(
            "xml.instruct! :xml, version: \"1.0\"\nxml.rss version: \"2.0\" do\n  xml.channel do\n    xml.tag! \"atom:link\", nil, rel: :self\n    xml.title \"A & B\"\n  end\nend\n",
        );
        assert_eq!(
            text_of(&out),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\">\n  <channel>\n    <atom:link rel=\"self\"/>\n    <title>A &amp; B</title>\n  </channel>\n</rss>\n"
        );
    }

    #[test]
    fn runtime_values_go_through_the_builder_escapes() {
        let out = compile_builder("xml.item do\n  xml.title story.title\n  xml.link href: story.url\nend\n");
        assert!(out.contains("ActionView::ViewHelpers.builder_text((story.title).to_s).html_safe"), "{out}");
        assert!(out.contains("ActionView::ViewHelpers.builder_attr((story.url).to_s).html_safe"), "{out}");
    }

    #[test]
    fn ruby_around_elements_is_copied_with_its_body_compiled() {
        let out = compile_builder(
            "@stories.each do |story|\n  xml.item do\n    xml.pubDate story.created_at if story.created_at\n  end\nend\n",
        );
        assert!(out.contains("@stories.each do |story|\n"), "{out}");
        assert!(out.contains("if story.created_at\n"), "{out}");
        assert!(ruby_prism::parse(out.as_bytes()).errors().next().is_none(), "{out}");
    }

    #[test]
    fn attribute_escape_covers_quotes_and_newlines() {
        assert_eq!(escape_attr("a\"b\nc<"), "a&quot;b&#10;c&lt;");
        assert_eq!(escape_text("\"q\""), "\"q\"");
    }
}
