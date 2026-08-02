//! Recognize a model's `as_json` body as an ORDERED LIST OF JSON PAIRS.
//!
//! This is the analysis half of monomorphizing inline `render json:`.
//! A strict target has no reflective encoder to fall back on, so the
//! only way to serialize a model is to know its key set at transpile
//! time. Rails apps do not declare that key set — they compute it, in
//! one of two idioms the corpus uses:
//!
//!   A. entry-list + walk (lobsters Story, Comment)
//!
//! ```text
//! h = [ :short_id, { comment_count: :comments_count },
//!       { tags: self.tags.map(&:tag).sort } ]
//! js = {}
//! h.each { |k| ... js[k] = self.send(k) ... }
//! js
//! ```
//!
//!   B. attrs + `super(only:)` + post-hoc writes (lobsters User, Message)
//!
//! ```text
//! attrs = [ :username, :created_at ]
//! attrs.push :karma if !self.is_admin?
//! h = super(only: attrs)          # → `_as_json_only` (as_json_super)
//! h[:avatar_url] = self.avatar_url
//! h
//! ```
//!
//! Both are STATEMENT SEQUENCES that build a shape, not declarations of
//! one, so recognizing them is an abstract interpretation over a small
//! closed statement language — not a general partial evaluator. The
//! output is a `Vec<JsonPair>` an emitter can turn into straight-line
//! `io << "\"k\":" << …` appends, the same shape
//! `jbuilder_to_library` already produces for `*.json.jbuilder`.
//!
//! WHAT THIS DELIBERATELY DOES NOT DO. It does not evaluate. A pair
//! whose presence depends on a runtime test (`karma` only for
//! non-admins) keeps that test as `JsonPair::cond` for the emitter to
//! wrap an `if` around — the key set stays row-dependent, because in
//! Rails it IS row-dependent. Folding it away would emit JSON that
//! differs from Rails.
//!
//! Anything outside the two idioms returns `Err`. The caller ledgers
//! that as modeling debt and leaves the arm dropped, which is what the
//! `respond_to` flattening already does today — so an unrecognized
//! `as_json` degrades to exactly the current behavior, never to wrong
//! JSON.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

/// How one pair's value is produced.
#[derive(Clone, Debug)]
pub enum PairValue {
    /// Call a reader on self: `self.<name>`. Both a bare `:sym` entry
    /// and a `{ key: :sym }` rename lower to this — the rename only
    /// changes the KEY, never how the value is fetched.
    Reader(Symbol),
    /// An expression written out in the source, used verbatim
    /// (`{ tags: self.tags.map(&:tag).sort }`).
    Computed(Expr),
}

/// One key/value pair in the serialized object, in source order.
#[derive(Clone, Debug)]
pub struct JsonPair {
    pub key: Symbol,
    pub value: PairValue,
    /// Runtime guard on this pair's PRESENCE (not its value). `None`
    /// for the common unconditional pair.
    pub cond: Option<Expr>,
}

/// Why an `as_json` body was declined. Carried into the caller's
/// diagnostic so the ledger says which construct blocked it rather
/// than just "unsupported".
pub type ShapeError = &'static str;

/// Recognize `body` as an ordered pair list, or explain why not.
pub fn as_json_pairs(body: &Expr) -> Result<Vec<JsonPair>, ShapeError> {
    let stmts = seq_stmts(body);
    if stmts.is_empty() {
        return Err("empty as_json body");
    }
    // The two idioms are told apart by what the built-up list feeds:
    // an `each` walk (A) or a `super(only:)` call (B). Both start by
    // assigning an array literal to a local, so dispatch on the tail
    // rather than the head.
    if find_only_call(&stmts).is_some() {
        pairs_from_attrs_idiom(&stmts)
    } else {
        pairs_from_entry_list_idiom(&stmts)
    }
}

// ── idiom B: attrs + super(only:) + post-hoc writes ────────────────

fn pairs_from_attrs_idiom(stmts: &[&Expr]) -> Result<Vec<JsonPair>, ShapeError> {
    let only_at = find_only_call(stmts).ok_or("no `super(only:)` call")?;
    let attrs_var = only_arg_local(stmts[only_at]).ok_or("`only:` argument is not a local")?;

    let mut pairs: Vec<JsonPair> = Vec::new();
    // Statements BEFORE the super() build the attribute list; every
    // one of them must be recognized, or the key set we derive is not
    // the key set Rails would produce.
    for stmt in &stmts[..only_at] {
        let (inner, cond) = split_guard(stmt);
        match &*inner.node {
            // `attrs = [ :a, :b ]`
            ExprNode::Assign { target: LValue::Var { name, .. }, value }
                if name == &attrs_var =>
            {
                if cond.is_some() {
                    return Err("conditional assignment to the attrs list");
                }
                for el in array_elements(value).ok_or("attrs list is not an array literal")? {
                    let sym = lit_sym(el).ok_or("attrs entry is not a symbol literal")?;
                    pairs.push(JsonPair { key: sym.clone(), value: PairValue::Reader(sym), cond: None });
                }
            }
            // `attrs.push :x` / `attrs.push :x, :y`, optionally guarded.
            ExprNode::Send { recv: Some(recv), method, args, .. }
                if method.as_str() == "push" && is_named_local(recv, &attrs_var) =>
            {
                for a in args {
                    let sym = lit_sym(a).ok_or("pushed attr is not a symbol literal")?;
                    pairs.push(JsonPair {
                        key: sym.clone(),
                        value: PairValue::Reader(sym),
                        cond: cond.cloned(),
                    });
                }
            }
            _ => return Err("unrecognized statement before `super(only:)`"),
        }
    }

    // The super() assigns the base hash; everything after it either
    // writes another pair or is the trailing read of that hash.
    let hash_var = assign_target_local(stmts[only_at]).ok_or("`super(only:)` result unassigned")?;
    for stmt in &stmts[only_at + 1..] {
        let (inner, cond) = split_guard(stmt);
        // Trailing `h` — the return.
        if is_named_local(inner, &hash_var) {
            continue;
        }
        match &*inner.node {
            // `h[:k] = v` reaches the IR as a `[]=` Send, not an
            // `Assign`/`LValue::Index` — index writes are ordinary
            // method calls once ingested.
            ExprNode::Send { recv: Some(recv), method, args, .. }
                if method.as_str() == "[]="
                    && args.len() == 2
                    && is_named_local(recv, &hash_var) =>
            {
                let key = lit_sym(&args[0]).ok_or("hash write key is not a symbol literal")?;
                pairs.push(JsonPair {
                    key,
                    value: PairValue::Computed(args[1].clone()),
                    cond: cond.cloned(),
                });
            }
            _ => return Err("unrecognized statement after `super(only:)`"),
        }
    }
    Ok(pairs)
}

// ── idiom A: entry list + each walk ────────────────────────────────

fn pairs_from_entry_list_idiom(stmts: &[&Expr]) -> Result<Vec<JsonPair>, ShapeError> {
    // The first array-literal assignment is the entry list. Its walk
    // is not interpreted: the `h.each` body is boilerplate that
    // dispatches on each entry's SHAPE, and we read the shapes
    // directly off the literal instead.
    let mut entries: Option<(Symbol, Vec<Expr>)> = None;
    for stmt in stmts {
        if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*stmt.node {
            if let Some(els) = array_elements(value) {
                entries = Some((name.clone(), els.to_vec()));
                break;
            }
        }
    }
    let (list_var, els) = entries.ok_or("no entry-list array literal")?;

    let mut pairs: Vec<JsonPair> = Vec::new();
    for el in &els {
        // Bare `:sym` — key and reader are the same name.
        if let Some(sym) = lit_sym(el) {
            pairs.push(JsonPair { key: sym.clone(), value: PairValue::Reader(sym), cond: None });
            continue;
        }
        // `{ key: <value> }` — exactly one entry per Rails' idiom.
        let (k, v) = single_hash_entry(el).ok_or("entry is neither a symbol nor a 1-key hash")?;
        let key = lit_sym(&k).ok_or("entry-hash key is not a symbol literal")?;
        // `{ comment_count: :comments_count }` renames the key and
        // reads a DIFFERENT reader; `{ tags: <expr> }` carries the
        // value itself. That is precisely the `is_a?(Symbol)` test the
        // walk performs at runtime — answered here, statically.
        let value = match lit_sym(&v) {
            Some(reader) => PairValue::Reader(reader),
            None => PairValue::Computed(v.clone()),
        };
        pairs.push(JsonPair { key, value, cond: None });
    }

    // A `push` onto the entry list after the literal would add pairs
    // this reader has not accounted for. Story guards one on
    // `options[:with_comments]`, which is false for the no-argument
    // `as_json` every `render json:` site calls — but proving that
    // belongs to the call-site specializer, not here, so decline and
    // let the caller ledger it rather than silently emit a key set
    // that is wrong whenever options are passed.
    // Scan the WHOLE body, not just top-level statements: Story's push
    // rides an `if` modifier, and a guard shape this reader does not
    // recognize must not be the reason a push slips past. Missing one
    // would mean emitting a key set that silently disagrees with Rails.
    for stmt in stmts {
        if contains_push_to(stmt, &list_var) {
            return Err("entry list is extended by `push` after the literal");
        }
    }
    Ok(pairs)
}

// ── shape helpers ──────────────────────────────────────────────────

/// Flatten a method body to its top-level statements.
fn seq_stmts(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

/// Peel a trailing-`if` guard: `expr if cond` parses as an `If` with an
/// empty else. Returns the guarded expression and the condition.
fn split_guard(stmt: &Expr) -> (&Expr, Option<&Expr>) {
    if let ExprNode::If { cond, then_branch, else_branch } = &*stmt.node {
        if is_empty_expr(else_branch) {
            return (then_branch, Some(cond));
        }
    }
    (stmt, None)
}

fn is_empty_expr(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Seq { exprs } => exprs.is_empty(),
        // An `expr if cond` modifier renders its missing else as nil on
        // some paths and as an empty Seq on others; both mean "no else".
        ExprNode::Lit { value: Literal::Nil } => true,
        _ => false,
    }
}

fn array_elements(e: &Expr) -> Option<&[Expr]> {
    match &*e.node {
        ExprNode::Array { elements, .. } => Some(elements),
        _ => None,
    }
}

fn lit_sym(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

fn single_hash_entry(e: &Expr) -> Option<(Expr, Expr)> {
    match &*e.node {
        ExprNode::Hash { entries, .. } if entries.len() == 1 => {
            Some((entries[0].0.clone(), entries[0].1.clone()))
        }
        _ => None,
    }
}

/// True when `e` reads as the named local. Prism renders a bare local
/// read as `Var`, but a receiverless argless `Send` shows up for names
/// it could not resolve to a local slot, so accept both.
fn is_named_local(e: &Expr, name: &Symbol) -> bool {
    match &*e.node {
        ExprNode::Var { name: n, .. } => n == name,
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            method == name && args.is_empty()
        }
        _ => false,
    }
}

/// Index of the statement assigning `super(only: …)`'s result. Runs
/// AFTER `as_json_super`, so the call is `_as_json_only(attrs)`; the
/// raw `Super` form is accepted too so this reads correctly on an
/// un-grounded body.
fn find_only_call(stmts: &[&Expr]) -> Option<usize> {
    stmts.iter().position(|s| {
        let value = match &*s.node {
            ExprNode::Assign { value, .. } => value,
            _ => return false,
        };
        match &*value.node {
            ExprNode::Send { recv: None, method, .. } => method.as_str() == "_as_json_only",
            ExprNode::Super { args: Some(_) } => true,
            _ => false,
        }
    })
}

fn only_arg_local(stmt: &Expr) -> Option<Symbol> {
    let value = match &*stmt.node {
        ExprNode::Assign { value, .. } => value,
        _ => return None,
    };
    let arg = match &*value.node {
        ExprNode::Send { args, .. } if args.len() == 1 => &args[0],
        ExprNode::Super { args: Some(args) } if args.len() == 1 => &args[0],
        _ => return None,
    };
    // `super(only: attrs)` hands over a one-entry kwarg Hash; the
    // `_as_json_only(attrs)` rewrite has already unwrapped it. Accept
    // both so this reads the same before and after `as_json_super`.
    let arg = match single_hash_entry(arg) {
        Some((k, v)) if lit_sym(&k).map(|s| s.as_str() == "only").unwrap_or(false) => {
            return local_name(&v)
        }
        Some(_) => return None,
        None => arg,
    };
    match &*arg.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.clone())
        }
        _ => None,
    }
}

/// Recursively true when `e` contains `<name>.push(...)` anywhere.
fn contains_push_to(e: &Expr, name: &Symbol) -> bool {
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*e.node {
        if method.as_str() == "push" && is_named_local(recv, name) {
            return true;
        }
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| {
        if !found && contains_push_to(c, name) {
            found = true;
        }
    });
    found
}

/// A bare local read, in either spelling Prism produces.
fn local_name(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.clone())
        }
        _ => None,
    }
}

fn assign_target_local(stmt: &Expr) -> Option<Symbol> {
    match &*stmt.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => Some(name.clone()),
        _ => None,
    }
}
