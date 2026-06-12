//! The one renderer for the JS/TS AST (`js_ast.rs`). Owns every
//! formatting decision the string emitter used to make inline:
//! parentheses (derived from the precedence table — never stored in
//! the tree), indentation (2 spaces), string/template escaping,
//! single-statement block inlining, and blank-line placement.
//!
//! Source maps: construct with [`Printer::with_mappings`] and every
//! node carrying a real span records a `(generated line, generated
//! col, span)` triple as it begins printing. The VLQ serializer
//! consumes those triples; this file knows nothing about the
//! source-map format itself.

use crate::span::Span;

use super::js_ast::{
    ArrowBody, Js, JsClassMember, JsDecl, JsExpr, JsImport, JsKey, JsModule, JsObjEntry, JsParam,
    JsStmt, JsStmtNode, MethodKind, TplPart, VarKind,
};

/// Render one expression as source text, no source-map collection.
/// Bridge for emit paths still composing strings; the module-level
/// emit goes through [`Printer::module`] instead.
pub(super) fn render_expr(e: &Js) -> String {
    let mut p = Printer::new();
    p.expr(e, Prec::LOWEST);
    p.out
}

/// Render a statement list as source text at indent 0, without a
/// trailing newline (the legacy `emit_body` contract — callers
/// re-indent and join lines themselves).
pub(super) fn render_stmts(stmts: &[JsStmt]) -> String {
    let mut p = Printer::new();
    for s in stmts {
        p.stmt(s);
    }
    let mut out = p.out;
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// One token-level source-map entry: the generated position where a
/// spanned node begins. 0-based line, 0-based column (the source-map
/// convention).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapping {
    pub gen_line: u32,
    pub gen_col: u32,
    pub span: Span,
}

/// Inline single-statement blocks only when the whole line stays
/// within this budget — past it, expand to a real block.
const INLINE_WIDTH: usize = 150;

pub struct Printer {
    out: String,
    indent: usize,
    /// Current 0-based output position, maintained on every write.
    line: u32,
    col: u32,
    mappings: Option<Vec<Mapping>>,
}

impl Printer {
    pub fn new() -> Self {
        Printer { out: String::new(), indent: 0, line: 0, col: 0, mappings: None }
    }

    pub fn with_mappings() -> Self {
        Printer { mappings: Some(Vec::new()), ..Printer::new() }
    }

    pub fn finish(self) -> (String, Vec<Mapping>) {
        (self.out, self.mappings.unwrap_or_default())
    }

    /// Render a whole module: header comments, imports, then
    /// declarations separated by blank lines.
    pub fn module(mut self, m: &JsModule) -> (String, Vec<Mapping>) {
        for h in &m.header {
            self.word("// ");
            self.word(h);
            self.newline();
        }
        for imp in &m.imports {
            self.import(imp);
        }
        if !m.imports.is_empty() || !m.header.is_empty() {
            self.newline();
        }
        for (i, d) in m.decls.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.decl(d);
        }
        self.finish()
    }

    // ── low-level writing ────────────────────────────────────────────

    fn word(&mut self, s: &str) {
        debug_assert!(!s.contains('\n'), "word() text must be newline-free: {s:?}");
        self.out.push_str(s);
        self.col += s.chars().count() as u32;
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.line += 1;
        self.col = 0;
    }

    fn start_line(&mut self) {
        let pad = "  ".repeat(self.indent);
        self.word(&pad);
    }

    fn mark(&mut self, span: Span) {
        if span.is_synthetic() {
            return;
        }
        if let Some(maps) = &mut self.mappings {
            maps.push(Mapping { gen_line: self.line, gen_col: self.col, span });
        }
    }

    // ── imports / declarations ───────────────────────────────────────

    fn import(&mut self, imp: &JsImport) {
        self.word("import { ");
        for (i, (name, alias)) in imp.names.iter().enumerate() {
            if i > 0 {
                self.word(", ");
            }
            self.word(name);
            if let Some(a) = alias {
                self.word(" as ");
                self.word(a);
            }
        }
        self.word(" } from \"");
        self.word(&imp.from);
        self.word("\";");
        self.newline();
    }

    fn decl(&mut self, d: &JsDecl) {
        match d {
            JsDecl::Function { export, is_async, name, params, ret, body, span } => {
                self.start_line();
                self.mark(*span);
                if *export {
                    self.word("export ");
                }
                if *is_async {
                    self.word("async ");
                }
                self.word("function ");
                self.word(name);
                self.params(params);
                self.return_ty(ret);
                self.word(" {");
                self.newline();
                self.indented_stmts(body);
                self.start_line();
                self.word("}");
                self.newline();
            }
            JsDecl::Class { export, name, extends, members, span } => {
                self.start_line();
                self.mark(*span);
                if *export {
                    self.word("export ");
                }
                self.word("class ");
                self.word(name);
                if let Some(parent) = extends {
                    self.word(" extends ");
                    self.word(parent);
                }
                self.word(" {");
                self.newline();
                self.indent += 1;
                for m in members {
                    self.class_member(m);
                }
                self.indent -= 1;
                self.start_line();
                self.word("}");
                self.newline();
            }
            JsDecl::Const { export, name, ty, value, span } => {
                self.start_line();
                self.mark(*span);
                if *export {
                    self.word("export ");
                }
                self.word("const ");
                self.word(name);
                if let Some(t) = ty {
                    self.word(": ");
                    self.word(&t.0);
                }
                self.word(" = ");
                self.expr(value, Prec::ASSIGN);
                self.word(";");
                self.newline();
            }
            JsDecl::TypeAlias { export, name, ty } => {
                self.start_line();
                if *export {
                    self.word("export ");
                }
                self.word("type ");
                self.word(name);
                self.word(" = ");
                self.word(&ty.0);
                self.word(";");
                self.newline();
            }
            JsDecl::Raw(text) => {
                for line in text.lines() {
                    self.start_line();
                    self.word(line);
                    self.newline();
                }
            }
        }
    }

    fn class_member(&mut self, m: &JsClassMember) {
        match m {
            JsClassMember::Field { is_static, declare, name, ty, init, span } => {
                self.start_line();
                self.mark(*span);
                if *is_static {
                    self.word("static ");
                }
                if *declare {
                    self.word("declare ");
                }
                self.word(name);
                if let Some(t) = ty {
                    self.word(": ");
                    self.word(&t.0);
                }
                if let Some(v) = init {
                    self.word(" = ");
                    self.expr(v, Prec::ASSIGN);
                }
                self.word(";");
                self.newline();
            }
            JsClassMember::Method { is_static, is_async, kind, name, params, ret, body, span } => {
                self.start_line();
                self.mark(*span);
                if *is_static {
                    self.word("static ");
                }
                if *is_async {
                    self.word("async ");
                }
                match kind {
                    MethodKind::Get => self.word("get "),
                    MethodKind::Set => self.word("set "),
                    MethodKind::Normal | MethodKind::Constructor => {}
                }
                self.word(name);
                self.params(params);
                if !matches!(kind, MethodKind::Set | MethodKind::Constructor) {
                    self.return_ty(ret);
                }
                self.word(" {");
                self.newline();
                self.indented_stmts(body);
                self.start_line();
                self.word("}");
                self.newline();
            }
            JsClassMember::Blank => self.newline(),
        }
    }

    fn params(&mut self, params: &[JsParam]) {
        self.word("(");
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                self.word(", ");
            }
            self.param(p);
        }
        self.word(")");
    }

    fn param(&mut self, p: &JsParam) {
        self.word(&p.name);
        if p.optional {
            self.word("?");
        }
        if let Some(t) = &p.ty {
            self.word(": ");
            self.word(&t.0);
        }
        if let Some(d) = &p.default {
            self.word(" = ");
            self.expr(d, Prec::ASSIGN);
        }
    }

    fn return_ty(&mut self, ret: &Option<super::js_ast::TsType>) {
        if let Some(t) = ret {
            self.word(": ");
            self.word(&t.0);
        }
    }

    // ── statements ───────────────────────────────────────────────────

    fn indented_stmts(&mut self, stmts: &[JsStmt]) {
        self.indent += 1;
        for s in stmts {
            self.stmt(s);
        }
        self.indent -= 1;
    }

    fn stmt(&mut self, s: &JsStmt) {
        match &*s.node {
            JsStmtNode::Blank => {
                self.newline();
                return;
            }
            JsStmtNode::Comment(lines) => {
                for l in lines {
                    self.start_line();
                    self.word("// ");
                    self.word(l);
                    self.newline();
                }
                return;
            }
            _ => {}
        }
        self.start_line();
        self.stmt_inline(s);
        self.newline();
    }

    /// Print one statement starting at the current position (no
    /// leading indent, no trailing newline). Multi-line statement
    /// forms (blocks that don't inline) still emit interior newlines.
    fn stmt_inline(&mut self, s: &JsStmt) {
        self.mark(s.span);
        match &*s.node {
            JsStmtNode::Expr(e) => {
                // An expression statement starting with `{` would parse
                // as a block — parenthesize object literals.
                if matches!(&*e.node, JsExpr::Object(_)) {
                    self.word("(");
                    self.expr(e, Prec::LOWEST);
                    self.word(")");
                } else {
                    self.expr(e, Prec::LOWEST);
                }
                self.word(";");
            }
            JsStmtNode::VarDecl { kind, name, ty, init } => {
                self.word(match kind {
                    VarKind::Const => "const ",
                    VarKind::Let => "let ",
                });
                self.word(name);
                if let Some(t) = ty {
                    self.word(": ");
                    self.word(&t.0);
                }
                if let Some(v) = init {
                    self.word(" = ");
                    self.expr(v, Prec::ASSIGN);
                }
                self.word(";");
            }
            JsStmtNode::Return(v) => {
                self.word("return");
                if let Some(e) = v {
                    self.word(" ");
                    self.expr(e, Prec::LOWEST);
                }
                self.word(";");
            }
            JsStmtNode::If { cond, then, else_ } => {
                self.word("if (");
                self.expr(cond, Prec::LOWEST);
                self.word(") ");
                self.block(then);
                if let Some(els) = else_ {
                    self.word(" else ");
                    // Single-`If` else bodies render as `else if`.
                    if let [only] = els.as_slice() {
                        if matches!(&*only.node, JsStmtNode::If { .. }) {
                            self.stmt_inline(only);
                            return;
                        }
                    }
                    self.block(els);
                }
            }
            JsStmtNode::While { cond, body } => {
                self.word("while (");
                self.expr(cond, Prec::LOWEST);
                self.word(") ");
                self.block(body);
            }
            JsStmtNode::ForOf { binding, iterable, body } => {
                self.word("for (const ");
                self.word(binding);
                self.word(" of ");
                self.expr(iterable, Prec::LOWEST);
                self.word(") ");
                self.block(body);
            }
            JsStmtNode::ForNum { binding, limit, body } => {
                self.word("for (let ");
                self.word(binding);
                self.word(" = 0; ");
                self.word(binding);
                self.word(" < ");
                self.expr(limit, Prec::LOWEST);
                self.word("; ");
                self.word(binding);
                self.word("++) ");
                self.block(body);
            }
            JsStmtNode::Switch { scrutinee, cases, default } => {
                self.word("switch (");
                self.expr(scrutinee, Prec::LOWEST);
                self.word(") {");
                self.newline();
                self.indent += 1;
                for (value, body) in cases {
                    self.start_line();
                    self.word("case ");
                    self.expr(value, Prec::LOWEST);
                    self.word(":");
                    self.case_body(body);
                }
                if let Some(body) = default {
                    self.start_line();
                    self.word("default:");
                    self.case_body(body);
                }
                self.indent -= 1;
                self.start_line();
                self.word("}");
            }
            JsStmtNode::Throw(e) => {
                self.word("throw ");
                self.expr(e, Prec::LOWEST);
                self.word(";");
            }
            JsStmtNode::Try { body, catch, finally } => {
                self.word("try ");
                self.expand_block(body);
                if let Some((binding, cbody)) = catch {
                    match binding {
                        Some(b) => {
                            self.word(" catch (");
                            self.word(b);
                            self.word(") ");
                        }
                        None => self.word(" catch "),
                    }
                    self.expand_block(cbody);
                }
                if let Some(fbody) = finally {
                    self.word(" finally ");
                    self.expand_block(fbody);
                }
            }
            JsStmtNode::Break => self.word("break;"),
            JsStmtNode::Continue => self.word("continue;"),
            JsStmtNode::Raw(text) => {
                let mut lines = text.lines();
                if let Some(first) = lines.next() {
                    self.word(first);
                }
                for l in lines {
                    self.newline();
                    self.start_line();
                    self.word(l);
                }
            }
            JsStmtNode::Blank | JsStmtNode::Comment(_) => unreachable!("handled in stmt()"),
        }
    }

    /// `case X:` body — inline a single short statement after the
    /// colon (`case "show": this.show(); break;`), expand otherwise.
    /// A trailing `break;` is added unless the body already diverges.
    fn case_body(&mut self, body: &[JsStmt]) {
        let diverges = matches!(
            body.last().map(|s| &*s.node),
            Some(JsStmtNode::Return(_)) | Some(JsStmtNode::Throw(_)) | Some(JsStmtNode::Break)
        );
        if let Some(inline) = self.try_inline(body) {
            self.word(" ");
            self.word(&inline);
            if !diverges {
                self.word(" break;");
            }
            self.newline();
            return;
        }
        self.newline();
        self.indented_stmts(body);
        if !diverges {
            self.indent += 1;
            self.start_line();
            self.word("break;");
            self.newline();
            self.indent -= 1;
        }
    }

    /// A `{ ... }` block in statement context: inline when the body is
    /// a single simple statement that fits the width budget, expanded
    /// otherwise.
    fn block(&mut self, stmts: &[JsStmt]) {
        if stmts.is_empty() {
            self.word("{}");
            return;
        }
        if stmts.len() == 1 {
            if let Some(inline) = self.try_inline(stmts) {
                self.word("{ ");
                // Re-render through the main printer so mapping
                // positions are recorded against the real output.
                self.stmt_inline(&stmts[0]);
                let _ = inline;
                self.word(" }");
                return;
            }
        }
        self.expand_block(stmts);
    }

    /// Arrow-function block body. Unlike statement blocks (which only
    /// inline a single statement), arrow bodies inline any statement
    /// run that fits the width budget — lambdas appear in expression
    /// position where vertical expansion costs the most readability.
    fn arrow_block(&mut self, stmts: &[JsStmt]) {
        if stmts.is_empty() {
            self.word("{}");
            return;
        }
        if self.try_inline(stmts).is_some() {
            self.word("{ ");
            for (i, s) in stmts.iter().enumerate() {
                if i > 0 {
                    self.word(" ");
                }
                self.stmt_inline(s);
            }
            self.word(" }");
            return;
        }
        self.expand_block(stmts);
    }

    /// Always-multi-line `{ ... }`.
    fn expand_block(&mut self, stmts: &[JsStmt]) {
        self.word("{");
        self.newline();
        self.indented_stmts(stmts);
        self.start_line();
        self.word("}");
    }

    /// Trial-render `stmts` as a single line. `Some(text)` when every
    /// statement renders newline-free and the result fits the width
    /// budget from the current column. Mappings are not recorded
    /// during trials.
    fn try_inline(&mut self, stmts: &[JsStmt]) -> Option<String> {
        let mut trial = Printer { indent: self.indent, ..Printer::new() };
        for (i, s) in stmts.iter().enumerate() {
            if i > 0 {
                trial.word(" ");
            }
            if matches!(&*s.node, JsStmtNode::Blank | JsStmtNode::Comment(_)) {
                return None;
            }
            trial.stmt_inline(s);
        }
        let text = trial.out;
        if text.contains('\n') {
            return None;
        }
        if self.col as usize + text.len() + 4 > INLINE_WIDTH {
            return None;
        }
        Some(text)
    }

    // ── expressions ──────────────────────────────────────────────────

    /// Print `e`, parenthesizing when its own precedence binds looser
    /// than the context requires.
    fn expr(&mut self, e: &Js, min: u8) {
        let prec = expr_prec(&e.node);
        if prec < min {
            self.word("(");
            self.mark(e.span);
            self.expr_node(e);
            self.word(")");
        } else {
            self.mark(e.span);
            self.expr_node(e);
        }
    }

    fn expr_node(&mut self, e: &Js) {
        match &*e.node {
            JsExpr::Ident(name) => self.word(name),
            JsExpr::Num(text) => self.word(text),
            JsExpr::Str(text) => {
                let escaped = escape_str(text);
                self.word(&escaped);
            }
            JsExpr::Bool(b) => self.word(if *b { "true" } else { "false" }),
            JsExpr::Null => self.word("null"),
            JsExpr::Template(parts) => {
                self.word("`");
                for p in parts {
                    match p {
                        TplPart::Text(t) => {
                            let escaped = escape_template_text(t);
                            self.word(&escaped);
                        }
                        TplPart::Expr(inner) => {
                            self.word("${");
                            self.expr(inner, Prec::LOWEST);
                            self.word("}");
                        }
                    }
                }
                self.word("`");
            }
            JsExpr::Regex { pattern, flags } => {
                self.word("/");
                self.word(pattern);
                self.word("/");
                self.word(flags);
            }
            JsExpr::Array(items) => {
                self.word("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.word(", ");
                    }
                    self.expr(item, Prec::ASSIGN);
                }
                self.word("]");
            }
            JsExpr::Object(entries) => {
                if entries.is_empty() {
                    self.word("{}");
                    return;
                }
                self.word("{ ");
                for (i, entry) in entries.iter().enumerate() {
                    if i > 0 {
                        self.word(", ");
                    }
                    match entry {
                        JsObjEntry::Prop(k, v) => {
                            match k {
                                JsKey::Ident(name) => self.word(name),
                                JsKey::Str(text) => {
                                    let escaped = escape_str(text);
                                    self.word(&escaped);
                                }
                            }
                            self.word(": ");
                            self.expr(v, Prec::ASSIGN);
                        }
                        JsObjEntry::Spread(inner) => {
                            self.word("...");
                            self.expr(inner, Prec::ASSIGN);
                        }
                    }
                }
                self.word(" }");
            }
            JsExpr::Arrow { params, body, is_async } => {
                if *is_async {
                    self.word("async ");
                }
                // Single bare parameter prints without parens — except
                // on async arrows, where `async x => x` trips enough
                // downstream tooling that the parenthesized form is
                // the boring choice.
                if let [p] = params.as_slice() {
                    if !is_async && p.ty.is_none() && p.default.is_none() && !p.optional {
                        self.word(&p.name);
                    } else {
                        self.params(params);
                    }
                } else {
                    self.params(params);
                }
                self.word(" => ");
                match body {
                    ArrowBody::Expr(inner) => {
                        // Object-literal bodies need parens (`=> ({...})`).
                        if matches!(&*inner.node, JsExpr::Object(_)) {
                            self.word("(");
                            self.expr(inner, Prec::LOWEST);
                            self.word(")");
                        } else {
                            self.expr(inner, Prec::ASSIGN);
                        }
                    }
                    ArrowBody::Block(stmts) => self.arrow_block(stmts),
                }
            }
            JsExpr::Call { callee, args } => {
                self.expr(callee, Prec::CALL);
                self.args(args);
            }
            JsExpr::New { callee, args } => {
                self.word("new ");
                // `new` binds its callee tighter than calls do — a
                // call in callee position must parenthesize.
                self.expr(callee, Prec::NEW_CALLEE);
                self.args(args);
            }
            JsExpr::Member { obj, prop } => {
                self.expr(obj, Prec::CALL);
                self.word(".");
                self.word(prop);
            }
            JsExpr::Index { obj, index } => {
                self.expr(obj, Prec::CALL);
                self.word("[");
                self.expr(index, Prec::LOWEST);
                self.word("]");
            }
            JsExpr::Binary { op, left, right } => {
                let prec = bin_prec(op);
                // `??` refuses to mix bare with `&&`/`||` — force
                // parens on logical children regardless of precedence.
                let force = |child: &Js| {
                    *op == "??"
                        && matches!(
                            &*child.node,
                            JsExpr::Binary { op: "&&", .. } | JsExpr::Binary { op: "||", .. }
                        )
                };
                if force(left) {
                    self.word("(");
                    self.expr(left, Prec::LOWEST);
                    self.word(")");
                } else {
                    self.expr(left, prec);
                }
                self.word(" ");
                self.word(op);
                self.word(" ");
                if force(right) {
                    self.word("(");
                    self.expr(right, Prec::LOWEST);
                    self.word(")");
                } else {
                    self.expr(right, prec + 1);
                }
            }
            JsExpr::Unary { op, operand } => {
                self.word(op);
                self.expr(operand, Prec::UNARY);
            }
            JsExpr::Ternary { cond, then, else_ } => {
                self.expr(cond, Prec::TERNARY + 1);
                self.word(" ? ");
                self.expr(then, Prec::TERNARY);
                self.word(" : ");
                self.expr(else_, Prec::TERNARY);
            }
            JsExpr::Assign { target, op, value } => {
                self.expr(target, Prec::CALL);
                self.word(" ");
                self.word(op);
                self.word(" ");
                self.expr(value, Prec::ASSIGN);
            }
            JsExpr::Cast { expr, ty } => {
                // Always parenthesized — `(x as T)` composes safely in
                // every context without precedence reasoning.
                self.word("(");
                self.expr(expr, Prec::LOWEST);
                self.word(" as ");
                self.word(&ty.0);
                self.word(")");
            }
            JsExpr::Spread(inner) => {
                self.word("...");
                self.expr(inner, Prec::ASSIGN);
            }
            JsExpr::Raw(text) => {
                let mut lines = text.lines();
                if let Some(first) = lines.next() {
                    self.word(first);
                }
                for l in lines {
                    self.newline();
                    self.start_line();
                    self.word(l);
                }
            }
        }
    }

    fn args(&mut self, args: &[Js]) {
        self.word("(");
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.word(", ");
            }
            self.expr(a, Prec::ASSIGN);
        }
        self.word(")");
    }
}

// ── precedence ───────────────────────────────────────────────────────

/// Precedence levels, MDN-numbered (higher binds tighter). Children
/// are rendered with the minimum precedence their context tolerates;
/// anything looser gets parenthesized.
struct Prec;

impl Prec {
    const LOWEST: u8 = 0;
    /// Assignment / arrow position: argument lists, RHS, defaults.
    const ASSIGN: u8 = 2;
    const TERNARY: u8 = 3;
    const UNARY: u8 = 15;
    /// Member/index/call receiver position.
    const CALL: u8 = 18;
    /// `new` callee position — one above CALL so calls parenthesize.
    const NEW_CALLEE: u8 = 19;
    const ATOM: u8 = 20;
}

fn bin_prec(op: &str) -> u8 {
    match op {
        "**" => 14,
        "*" | "/" | "%" => 13,
        "+" | "-" => 12,
        "<<" | ">>" | ">>>" => 11,
        "<" | "<=" | ">" | ">=" | "instanceof" | "in" => 10,
        "==" | "!=" | "===" | "!==" => 9,
        "&" => 8,
        "^" => 7,
        "|" => 6,
        "&&" => 5,
        "||" | "??" => 4,
        other => panic!("unknown binary operator {other:?}"),
    }
}

fn expr_prec(e: &JsExpr) -> u8 {
    match e {
        JsExpr::Ident(_)
        | JsExpr::Num(_)
        | JsExpr::Str(_)
        | JsExpr::Bool(_)
        | JsExpr::Null
        | JsExpr::Template(_)
        | JsExpr::Regex { .. }
        | JsExpr::Array(_)
        | JsExpr::Object(_)
        // Cast prints its own parens; Raw splices pre-rendered text
        // that is never re-wrapped.
        | JsExpr::Cast { .. }
        | JsExpr::Raw(_) => Prec::ATOM,
        JsExpr::Call { .. } | JsExpr::Member { .. } | JsExpr::Index { .. } => Prec::CALL,
        JsExpr::New { .. } => Prec::CALL,
        JsExpr::Unary { .. } => Prec::UNARY,
        JsExpr::Binary { op, .. } => bin_prec(op),
        JsExpr::Ternary { .. } => Prec::TERNARY,
        JsExpr::Assign { .. } | JsExpr::Arrow { .. } => Prec::ASSIGN,
        // `...x` is only legal in arg/array/object positions, where it
        // must never be parenthesized (`(...x)` is a syntax error).
        JsExpr::Spread(_) => Prec::ATOM,
    }
}

// ── escaping ─────────────────────────────────────────────────────────

/// Double-quoted string literal with the escape set the string
/// emitter used (`\\`, `\"`, `\n`, `\r`, `\t`). Remaining control
/// characters take the ES6 `\u{...}` form — raw control bytes in a
/// literal are at best unreadable and at worst corrupted by
/// re-indenters (json_builder's `"\u{8}" => "\\b"` escape map is the
/// shipping case).
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                out.push_str(&format!("\\u{{{:x}}}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Template-literal text chunk: escape backticks, `${`, backslashes,
/// and the control characters whose literal form would change the
/// rendered output's indentation (the latent-indent-corruption fix
/// from the coalescing era — escaping `\n` keeps one template line
/// per source line).
fn escape_template_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::js_ast::*;
    use super::*;
    use crate::span::FileId;

    fn print_expr(e: &Js) -> String {
        let mut p = Printer::new();
        p.expr(e, Prec::LOWEST);
        p.out
    }

    fn print_stmt(s: &JsStmt) -> String {
        let mut p = Printer::new();
        p.stmt(s);
        p.out
    }

    fn ident(s: &str) -> Js {
        Js::synth(JsExpr::Ident(s.into()))
    }

    fn num(s: &str) -> Js {
        Js::synth(JsExpr::Num(s.into()))
    }

    #[test]
    fn binary_parens_follow_precedence() {
        // (a + b) * c — left child looser than * context.
        let e = Js::synth(JsExpr::Binary {
            op: "*",
            left: Js::synth(JsExpr::Binary { op: "+", left: ident("a"), right: ident("b") }),
            right: ident("c"),
        });
        assert_eq!(print_expr(&e), "(a + b) * c");
        // a + b * c — no parens needed.
        let e = Js::synth(JsExpr::Binary {
            op: "+",
            left: ident("a"),
            right: Js::synth(JsExpr::Binary { op: "*", left: ident("b"), right: ident("c") }),
        });
        assert_eq!(print_expr(&e), "a + b * c");
    }

    #[test]
    fn left_assoc_same_prec_right_child_parens() {
        // a - (b - c): right child of `-` at equal precedence wraps.
        let e = Js::synth(JsExpr::Binary {
            op: "-",
            left: ident("a"),
            right: Js::synth(JsExpr::Binary { op: "-", left: ident("b"), right: ident("c") }),
        });
        assert_eq!(print_expr(&e), "a - (b - c)");
        // (a - b) - c renders without parens.
        let e = Js::synth(JsExpr::Binary {
            op: "-",
            left: Js::synth(JsExpr::Binary { op: "-", left: ident("a"), right: ident("b") }),
            right: ident("c"),
        });
        assert_eq!(print_expr(&e), "a - b - c");
    }

    #[test]
    fn nullish_refuses_bare_logical_children() {
        let e = Js::synth(JsExpr::Binary {
            op: "??",
            left: Js::synth(JsExpr::Binary { op: "||", left: ident("a"), right: ident("b") }),
            right: ident("c"),
        });
        assert_eq!(print_expr(&e), "(a || b) ?? c");
    }

    #[test]
    fn unary_of_binary_parenthesizes() {
        let e = Js::synth(JsExpr::Unary {
            op: "!",
            operand: Js::synth(JsExpr::Binary {
                op: "===",
                left: ident("a"),
                right: ident("b"),
            }),
        });
        assert_eq!(print_expr(&e), "!(a === b)");
    }

    #[test]
    fn member_of_ternary_parenthesizes() {
        let e = Js::member(
            Span::synthetic(),
            Js::synth(JsExpr::Ternary {
                cond: ident("c"),
                then: ident("a"),
                else_: ident("b"),
            }),
            "title",
        );
        assert_eq!(print_expr(&e), "(c ? a : b).title");
    }

    #[test]
    fn new_callee_call_parenthesizes() {
        let e = Js::synth(JsExpr::New {
            callee: Js::call(Span::synthetic(), ident("f"), vec![]),
            args: vec![],
        });
        assert_eq!(print_expr(&e), "new (f())()");
        let plain = Js::synth(JsExpr::New { callee: ident("Article"), args: vec![] });
        assert_eq!(print_expr(&plain), "new Article()");
    }

    #[test]
    fn string_and_template_escaping() {
        let s = Js::str(Span::synthetic(), "a\"b\\c\nd");
        assert_eq!(print_expr(&s), r#""a\"b\\c\nd""#);
        let t = Js::synth(JsExpr::Template(vec![
            TplPart::Text("<a href=\"".into()),
            TplPart::Expr(ident("url")),
            TplPart::Text("\">`${x}\n".into()),
        ]));
        assert_eq!(print_expr(&t), "`<a href=\"${url}\">\\`\\${x}\\n`");
    }

    #[test]
    fn arrow_forms() {
        // Bare single param + expression body.
        let e = Js::synth(JsExpr::Arrow {
            params: vec![JsParam { name: "a".into(), optional: false, ty: None, default: None }],
            body: ArrowBody::Expr(Js::member(Span::synthetic(), ident("a"), "id")),
            is_async: false,
        });
        assert_eq!(print_expr(&e), "a => a.id");
        // Object body parenthesizes.
        let e = Js::synth(JsExpr::Arrow {
            params: vec![],
            body: ArrowBody::Expr(Js::synth(JsExpr::Object(vec![JsObjEntry::Prop(
                JsKey::Ident("a".into()),
                num("1"),
            )]))),
            is_async: false,
        });
        assert_eq!(print_expr(&e), "() => ({ a: 1 })");
    }

    #[test]
    fn single_statement_blocks_inline() {
        let body = vec![JsStmt::expr(Js::method_call(
            Span::synthetic(),
            ident("io"),
            "push",
            vec![Js::str(Span::synthetic(), "x")],
        ))];
        let s = JsStmt::synth(JsStmtNode::While { cond: ident("more"), body });
        assert_eq!(print_stmt(&s), "while (more) { io.push(\"x\"); }\n");
    }

    #[test]
    fn multi_statement_blocks_expand() {
        let body = vec![
            JsStmt::expr(Js::call(Span::synthetic(), ident("f"), vec![])),
            JsStmt::expr(Js::call(Span::synthetic(), ident("g"), vec![])),
        ];
        let s = JsStmt::synth(JsStmtNode::If { cond: ident("c"), then: body, else_: None });
        assert_eq!(print_stmt(&s), "if (c) {\n  f();\n  g();\n}\n");
    }

    #[test]
    fn else_if_chains_flat() {
        let inner = JsStmt::synth(JsStmtNode::If {
            cond: ident("b"),
            then: vec![JsStmt::expr(Js::call(Span::synthetic(), ident("g"), vec![]))],
            else_: None,
        });
        let s = JsStmt::synth(JsStmtNode::If {
            cond: ident("a"),
            then: vec![JsStmt::expr(Js::call(Span::synthetic(), ident("f"), vec![]))],
            else_: Some(vec![inner]),
        });
        assert_eq!(print_stmt(&s), "if (a) { f(); } else if (b) { g(); }\n");
    }

    #[test]
    fn switch_cases_inline_with_break() {
        let s = JsStmt::synth(JsStmtNode::Switch {
            scrutinee: ident("action"),
            cases: vec![(
                Js::str(Span::synthetic(), "index"),
                vec![JsStmt::expr(Js::method_call(
                    Span::synthetic(),
                    Js::this(),
                    "index",
                    vec![],
                ))],
            )],
            default: None,
        });
        assert_eq!(
            print_stmt(&s),
            "switch (action) {\n  case \"index\": this.index(); break;\n}\n"
        );
    }

    #[test]
    fn mappings_record_generated_positions() {
        let span = Span { file: FileId(1), start: 10, end: 20 };
        let inner = Js::method_call(span, ident("io"), "push", vec![Js::str(span, "x")]);
        let body = vec![JsStmt::expr(inner)];
        let m = JsModule {
            header: vec![],
            imports: vec![],
            decls: vec![JsDecl::Function {
                export: true,
                is_async: false,
                name: "index".into(),
                params: vec![],
                ret: Some(TsType("string".into())),
                body,
                span: Span::synthetic(),
            }],
        };
        let (text, maps) = Printer::with_mappings().module(&m);
        assert_eq!(text, "export function index(): string {\n  io.push(\"x\");\n}\n");
        // The statement + call + string literal all map; the first
        // entry points at line 1 col 2 (0-based), where `io` begins.
        assert!(!maps.is_empty());
        assert_eq!(maps[0].gen_line, 1);
        assert_eq!(maps[0].gen_col, 2);
        assert_eq!(maps[0].span, span);
    }

    #[test]
    fn trial_render_does_not_leak_mappings() {
        let span = Span { file: FileId(1), start: 0, end: 5 };
        let body = vec![JsStmt::expr(Js::call(span, ident("f"), vec![]))];
        let s = JsStmt::synth(JsStmtNode::While { cond: ident("c"), body });
        let mut p = Printer::with_mappings();
        p.stmt(&s);
        let (text, maps) = p.finish();
        assert_eq!(text, "while (c) { f(); }\n");
        // Exactly the real-render mappings: one for the ExprStmt and
        // one for the call — the trial pass records nothing.
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].gen_col, 12);
    }
}
