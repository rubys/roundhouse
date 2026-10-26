//! `CONST.each do |key, value| define_method(:"#{key}_ids") { … } end`
//! in a class body, unrolled into the `def`s it makes.
//!
//! Ruby runs the loop when the class body does, so the methods exist by
//! name only at run time. Over a constant the same body assigns a literal
//! Array or Hash (or a literal Array written in place), the names are
//! known now: one `def` per element, its name the template with the
//! element's key, its body the block's with the loop's parameters read as
//! their values. The key is the literal it is. The Hash value is read
//! back as `CONST.fetch(key)`, the object the closure captured, so its
//! identity and frozenness are Ruby's; the literal has the key, so `fetch`
//! answers what `[]` would, without the nil `[]` could.
//!
//! What the substitution cannot keep is left alone: a loop it does not
//! recognize stays the statement it was. Refused shapes: a constant
//! whose value is not a literal, a name the key does not decide, a
//! parameter list other than plain required ones, a block that writes a
//! loop parameter or reaches a local outside the loop, `yield` (the
//! defining method's block, not the new method's) and a bare `super`
//! (Ruby refuses implicit arguments from a `define_method` body).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use ruby_prism::{CallNode, Node};

use crate::dialect::{MethodDef, MethodReceiver, Param};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

use super::util::{constant_id_str, flatten_statements, node_span};
use super::ingest_expr;

/// One element's key: what the loop parameter reads as.
#[derive(Clone, Debug)]
enum Key {
    Sym(String),
    Str(String),
}

impl Key {
    fn text(&self) -> &str {
        match self {
            Key::Sym(s) | Key::Str(s) => s,
        }
    }

    fn expr(&self, span: Span) -> Expr {
        let value = match self {
            Key::Sym(s) => Literal::Sym { value: Symbol::from(s.as_str()) },
            Key::Str(s) => Literal::Str { value: s.clone() },
        };
        Expr::new(span, ExprNode::Lit { value })
    }
}

#[derive(Clone, Debug)]
enum Entries {
    Array(Vec<Key>),
    Hash(Vec<Key>),
}

/// The literal constants one class or module body assigns, by name.
#[derive(Clone, Debug, Default)]
pub(super) struct ConstTable {
    owner: Vec<String>,
    map: HashMap<String, Entries>,
}

impl ConstTable {
    pub(super) fn of(owner: &ClassId, statements: &[Node<'_>]) -> Self {
        let mut map = HashMap::new();
        for stmt in statements {
            let Some(write) = stmt.as_constant_write_node() else { continue };
            if let Some(entries) = literal_entries(&write.value()) {
                map.insert(constant_id_str(&write.name()).to_string(), entries);
            }
        }
        ConstTable { owner: owner.0.as_str().split("::").map(str::to_string).collect(), map }
    }
}

thread_local! {
    /// The enclosing bodies' tables, innermost last: a `class << self` or
    /// an `included do` body reads its module's constants lexically.
    static SCOPES: RefCell<Vec<ConstTable>> = const { RefCell::new(Vec::new()) };
}

/// Keeps `table` in scope until dropped.
pub(super) struct Scope;

impl Scope {
    pub(super) fn enter(table: ConstTable) -> Self {
        SCOPES.with(|s| s.borrow_mut().push(table));
        Scope
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        SCOPES.with(|s| s.borrow_mut().pop());
    }
}

/// The constant's qualified path and entries, innermost scope first.
fn lookup(name: &str) -> Option<(Vec<String>, Entries)> {
    SCOPES.with(|s| {
        s.borrow().iter().rev().find_map(|t| {
            t.map.get(name).map(|e| {
                let mut path = t.owner.clone();
                path.push(name.to_string());
                (path, e.clone())
            })
        })
    })
}

fn literal_key(node: &Node<'_>) -> Option<Key> {
    if let Some(sym) = node.as_symbol_node() {
        return Some(Key::Sym(String::from_utf8_lossy(sym.unescaped()).into_owned()));
    }
    let s = node.as_string_node()?;
    Some(Key::Str(String::from_utf8_lossy(s.unescaped()).into_owned()))
}

fn literal_entries(value: &Node<'_>) -> Option<Entries> {
    // `{ … }.freeze` / `%i[…].freeze`
    if let Some(call) = value.as_call_node() {
        if constant_id_str(&call.name()) != "freeze" || call.arguments().is_some() || call.block().is_some() {
            return None;
        }
        return literal_entries(&call.receiver()?);
    }
    if let Some(array) = value.as_array_node() {
        return array.elements().iter().map(|e| literal_key(&e)).collect::<Option<Vec<_>>>().map(Entries::Array);
    }
    let hash = value.as_hash_node()?;
    hash.elements()
        .iter()
        .map(|e| literal_key(&e.as_assoc_node()?.key()))
        .collect::<Option<Vec<_>>>()
        .map(Entries::Hash)
}

/// A recognized loop: what it iterates and the `define_method`s it runs.
struct Plan<'pr> {
    /// The constant as written, read back where the loop reads it (its
    /// span is a source reference the resolver knows). `None` for a
    /// literal Array written in place.
    constant: Option<(String, Span)>,
    entries: Entries,
    key_param: String,
    value_param: Option<String>,
    defines: Vec<CallNode<'pr>>,
}

fn plan<'pr>(stmt: &Node<'pr>, file: &str, named: &HashSet<String>) -> Option<Plan<'pr>> {
    let call = stmt.as_call_node()?;
    if constant_id_str(&call.name()) != "each" || call.arguments().is_some() {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    let recv = call.receiver()?;
    let (constant, entries) = if let Some(read) = recv.as_constant_read_node() {
        let name = constant_id_str(&read.name()).to_string();
        let (_, entries) = lookup(&name)?;
        (Some((name, node_span(&recv, file))), entries)
    } else {
        (None, Entries::Array(literal_entries(&recv).and_then(|e| match e {
            Entries::Array(keys) => Some(keys),
            Entries::Hash(_) => None,
        })?))
    };
    let names = required_params(block.parameters())?;
    let (key_param, value_param) = match (&entries, names.as_slice()) {
        (Entries::Array(_), [key]) => (key.clone(), None),
        (Entries::Hash(_), [key, value]) => (key.clone(), Some(value.clone())),
        _ => return None,
    };
    let body = block.body()?;
    let defines: Vec<CallNode<'pr>> = flatten_statements(body)
        .into_iter()
        .map(|s| {
            let c = s.as_call_node()?;
            (c.receiver().is_none()
                && constant_id_str(&c.name()) == "define_method"
                && c.arguments().is_some_and(|a| a.arguments().iter().count() == 1)
                && c.block().is_some_and(|b| b.as_block_node().is_some()))
            .then_some(c)
        })
        .collect::<Option<Vec<_>>>()?;
    if defines.is_empty() {
        return None;
    }
    let plan = Plan { constant, entries, key_param, value_param, defines };
    plan.defines.iter().all(|d| define_ok(d, &plan, named)).then_some(plan)
}

/// Plain required parameter names, or `None` for any other shape.
fn required_params(params: Option<Node<'_>>) -> Option<Vec<String>> {
    let Some(params) = params else { return Some(Vec::new()) };
    let params = params.as_block_parameters_node()?;
    if params.locals().iter().next().is_some() {
        return None;
    }
    let Some(list) = params.parameters() else { return Some(Vec::new()) };
    if list.optionals().iter().next().is_some()
        || list.rest().is_some()
        || list.posts().iter().next().is_some()
        || list.keywords().iter().next().is_some()
        || list.keyword_rest().is_some()
        || list.block().is_some()
    {
        return None;
    }
    list.requireds()
        .iter()
        .map(|r| Some(constant_id_str(&r.as_required_parameter_node()?.name()).to_string()))
        .collect()
}

fn define_ok(define: &CallNode<'_>, plan: &Plan<'_>, named: &HashSet<String>) -> bool {
    let Some(block) = define.block().and_then(|b| b.as_block_node()) else { return false };
    let Some(params) = required_params(block.parameters()) else { return false };
    let loop_params: Vec<&str> = std::iter::once(plan.key_param.as_str()).chain(plan.value_param.as_deref()).collect();
    if params.iter().any(|p| loop_params.contains(&p.as_str())) {
        return false;
    }
    let names = plan_names(define, plan);
    // A name a `private :name`-style marker in the same body sets the
    // visibility of is not one this tracks: refused, not left public.
    if names.as_ref().is_none_or(|n| {
        n.iter().any(|name| !super::model_macros::valid_def_name(&Symbol::from(name.as_str())) || named.contains(name))
    }) {
        return false;
    }
    let mut check = BodyCheck { depth: 0, loops: 0, loop_params: &loop_params, ok: true };
    if let Some(body) = block.body() {
        ruby_prism::Visit::visit(&mut check, &body);
    }
    check.ok
}

/// The name each element gives, or `None` when the key does not decide it.
fn plan_names(define: &CallNode<'_>, plan: &Plan<'_>) -> Option<Vec<String>> {
    let arg = define.arguments()?.arguments().iter().next()?;
    let keys = match &plan.entries {
        Entries::Array(k) | Entries::Hash(k) => k,
    };
    let mut names: Vec<String> = Vec::new();
    for key in keys {
        let name = name_for(&arg, &plan.key_param, key.text())?;
        if names.contains(&name) {
            return None;
        }
        names.push(name);
    }
    Some(names)
}

fn name_for(arg: &Node<'_>, key_param: &str, key: &str) -> Option<String> {
    let is_key = |n: &Node<'_>| n.as_local_variable_read_node().is_some_and(|r| r.depth() == 0 && constant_id_str(&r.name()) == key_param);
    if is_key(arg) {
        return Some(key.to_string());
    }
    let parts: Vec<Node<'_>> = if let Some(s) = arg.as_interpolated_symbol_node() {
        s.parts().iter().collect()
    } else if let Some(s) = arg.as_interpolated_string_node() {
        s.parts().iter().collect()
    } else {
        return None;
    };
    let mut out = String::new();
    let mut used_key = false;
    for part in parts {
        if let Some(s) = part.as_string_node() {
            out.push_str(&String::from_utf8_lossy(s.unescaped()));
        } else {
            let embedded = part.as_embedded_statements_node()?;
            let stmts: Vec<Node<'_>> = embedded.statements()?.body().iter().collect();
            let [only] = stmts.as_slice() else { return None };
            if !is_key(only) {
                return None;
            }
            out.push_str(key);
            used_key = true;
        }
    }
    used_key.then_some(out)
}

/// A `define_method` body the substitution keeps: loop parameters are
/// only read, no other outer local is reached, and nothing in it means
/// something else once it is a `def`.
struct BodyCheck<'a> {
    /// Blocks and lambdas entered inside the `define_method` block.
    depth: u32,
    /// `while` / `until` / `for` loops entered inside it.
    loops: u32,
    loop_params: &'a [&'a str],
    ok: bool,
}

impl BodyCheck<'_> {
    /// Whether a local `depth` scopes up from the current one lies outside
    /// the `define_method` block, and whether that is the loop's block.
    fn outer(&self, depth: u32) -> Option<bool> {
        (depth > self.depth).then_some(depth == self.depth + 1)
    }

    fn write(&mut self, depth: u32) {
        if self.outer(depth).is_some() {
            self.ok = false;
        }
    }
}

impl<'pr> ruby_prism::Visit<'pr> for BodyCheck<'_> {
    fn visit_local_variable_read_node(&mut self, node: &ruby_prism::LocalVariableReadNode<'pr>) {
        match self.outer(node.depth()) {
            Some(true) if self.loop_params.contains(&constant_id_str(&node.name())) => {}
            Some(_) => self.ok = false,
            None => {}
        }
    }
    fn visit_local_variable_write_node(&mut self, node: &ruby_prism::LocalVariableWriteNode<'pr>) {
        self.write(node.depth());
        ruby_prism::visit_local_variable_write_node(self, node);
    }
    fn visit_local_variable_operator_write_node(&mut self, node: &ruby_prism::LocalVariableOperatorWriteNode<'pr>) {
        self.write(node.depth());
        ruby_prism::visit_local_variable_operator_write_node(self, node);
    }
    fn visit_local_variable_or_write_node(&mut self, node: &ruby_prism::LocalVariableOrWriteNode<'pr>) {
        self.write(node.depth());
        ruby_prism::visit_local_variable_or_write_node(self, node);
    }
    fn visit_local_variable_and_write_node(&mut self, node: &ruby_prism::LocalVariableAndWriteNode<'pr>) {
        self.write(node.depth());
        ruby_prism::visit_local_variable_and_write_node(self, node);
    }
    fn visit_local_variable_target_node(&mut self, node: &ruby_prism::LocalVariableTargetNode<'pr>) {
        self.write(node.depth());
    }
    fn visit_yield_node(&mut self, _: &ruby_prism::YieldNode<'pr>) {
        self.ok = false;
    }
    fn visit_forwarding_super_node(&mut self, _: &ruby_prism::ForwardingSuperNode<'pr>) {
        self.ok = false;
    }
    fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
        self.ok = false;
    }
    // `next` / `break` / `redo` at the block's own level leave the
    // `define_method` block; inside a `def` they do not parse.
    fn visit_next_node(&mut self, node: &ruby_prism::NextNode<'pr>) {
        if self.depth == 0 && self.loops == 0 {
            self.ok = false;
        }
        ruby_prism::visit_next_node(self, node);
    }
    fn visit_break_node(&mut self, node: &ruby_prism::BreakNode<'pr>) {
        if self.depth == 0 && self.loops == 0 {
            self.ok = false;
        }
        ruby_prism::visit_break_node(self, node);
    }
    fn visit_redo_node(&mut self, _: &ruby_prism::RedoNode<'pr>) {
        if self.depth == 0 && self.loops == 0 {
            self.ok = false;
        }
    }
    fn visit_while_node(&mut self, node: &ruby_prism::WhileNode<'pr>) {
        self.loops += 1;
        ruby_prism::visit_while_node(self, node);
        self.loops -= 1;
    }
    fn visit_until_node(&mut self, node: &ruby_prism::UntilNode<'pr>) {
        self.loops += 1;
        ruby_prism::visit_until_node(self, node);
        self.loops -= 1;
    }
    fn visit_for_node(&mut self, node: &ruby_prism::ForNode<'pr>) {
        self.loops += 1;
        ruby_prism::visit_for_node(self, node);
        self.loops -= 1;
    }
    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
        // A nested block naming a loop parameter would shadow it.
        if node.locals().iter().any(|l| self.loop_params.contains(&constant_id_str(&l))) {
            self.ok = false;
        }
        self.depth += 1;
        ruby_prism::visit_block_node(self, node);
        self.depth -= 1;
    }
    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
        if node.locals().iter().any(|l| self.loop_params.contains(&constant_id_str(&l))) {
            self.ok = false;
        }
        self.depth += 1;
        ruby_prism::visit_lambda_node(self, node);
        self.depth -= 1;
    }
}

/// Whether `stmt` is a loop this unrolls, with the enclosing bodies' tables
/// in scope.
pub(super) fn unrollable(stmt: &Node<'_>, named: &HashSet<String>) -> bool {
    plan(stmt, "", named).is_some()
}

/// The names `private :a, :b`-style markers in `statements` set the
/// visibility of.
pub(super) fn named_visibility(statements: &[Node<'_>]) -> HashSet<String> {
    statements
        .iter()
        .filter_map(|s| s.as_call_node())
        .filter(|c| c.receiver().is_none() && super::visibility::marker(c))
        .flat_map(|c| c.arguments().map(|a| a.arguments().iter().filter_map(|n| literal_key(&n).map(|k| k.text().to_string())).collect::<Vec<_>>()).unwrap_or_default())
        .collect()
}

/// The `def`s the loop makes, or `None` when it is not one this unrolls.
pub(super) fn unroll(
    stmt: &Node<'_>,
    owner: &ClassId,
    file: &str,
    receiver: MethodReceiver,
    named: &HashSet<String>,
) -> Option<Vec<MethodDef>> {
    let plan = plan(stmt, file, named)?;
    let keys = match &plan.entries {
        Entries::Array(k) | Entries::Hash(k) => k.clone(),
    };
    let mut out = Vec::new();
    for define in &plan.defines {
        let names = plan_names(define, &plan)?;
        let block = define.block()?.as_block_node()?;
        let params: Vec<Param> = required_params(block.parameters())?
            .into_iter()
            .map(|p| Param::positional(Symbol::from(p.as_str())))
            .collect();
        let name_node = define.arguments()?.arguments().iter().next()?;
        let name_span = node_span(&name_node, file);
        let body = match block.body() {
            // A body it cannot ingest leaves the loop to the walk, as an
            // unrecognized loop is.
            Some(b) => ingest_expr(&b, file).ok()?,
            None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        };
        for (key, name) in keys.iter().zip(names) {
            let mut body = body.clone();
            substitute(&mut body, &plan, key);
            out.push(MethodDef {
                name_span,
                name: Symbol::from(name.as_str()),
                receiver,
                visibility: crate::dialect::MethodVisibility::Public,
                params: params.clone(),
                unsupported_formals: None,
                has_anonymous_block: false,
                body,
                signature: None,
                effects: crate::effect::EffectSet::default(),
                enclosing_class: Some(owner.0.clone()),
                kind: crate::dialect::AccessorKind::Method,
                is_async: false,
                mutates_self: false,
                block_param: None,
            });
        }
    }
    Some(out)
}

/// Each read of a loop parameter becomes the element's key, or for the
/// Hash value `CONST.fetch(key)`. `BodyCheck` proved no nested scope rebinds them.
fn substitute(expr: &mut Expr, plan: &Plan<'_>, key: &Key) {
    if let ExprNode::Var { name, .. } = &*expr.node {
        let span = expr.span;
        if name.as_str() == plan.key_param {
            *expr = key.expr(span);
            return;
        }
        if plan.value_param.as_deref() == Some(name.as_str())
            && let Some((constant, constant_span)) = &plan.constant
        {
            // The constant read where the loop reads it: its span is a
            // source reference, so it resolves (and a concern splice
            // qualifies it) as the loop's own receiver does.
            let recv = Expr::new(*constant_span, ExprNode::Const { path: vec![Symbol::from(constant.as_str())] });
            *expr = Expr::new(
                span,
                ExprNode::Send { recv: Some(recv), method: Symbol::from("fetch"), args: vec![key.expr(span)], block: None, parenthesized: true },
            );
            return;
        }
    }
    expr.node.for_each_child_mut(&mut |child| substitute(child, plan, key));
}

/// The spans an unrolled loop's methods carry (the `define_method` name
/// argument), for a scan that records a carrier's methods by span. Shape
/// only: a loop that does not unroll defines no method with these spans.
pub(super) fn name_spans(stmt: &Node<'_>, file: &str) -> Vec<Span> {
    let Some(call) = stmt.as_call_node() else { return Vec::new() };
    if constant_id_str(&call.name()) != "each" {
        return Vec::new();
    }
    let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) else { return Vec::new() };
    flatten_statements(body)
        .iter()
        .filter_map(|s| {
            let c = s.as_call_node()?;
            (constant_id_str(&c.name()) == "define_method").then(|| c.arguments()?.arguments().iter().next().map(|a| node_span(&a, file)))?
        })
        .collect()
}
