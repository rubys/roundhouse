//! IR method body / value → Elixir.
//!
//! Phase 2 walker, grown to cover `json_builder.rb`. Elixir is
//! expression-oriented and immutable, so two Ruby constructs need real
//! transformation rather than 1:1 syntax mapping:
//!
//! - **`return` elimination.** Elixir has no `return`. A guard-clause
//!   sequence (`return X if c1; return Y if c2; Z`) folds into nested
//!   `if c1, do: X, else: (if c2 …)`. `emit_stmts` does this by putting
//!   the rest of the block in the `else` branch of each guard.
//! - **Conditional local reassignment.** A variable reassigned inside an
//!   `if` body doesn't leak out in Elixir. `ms = "000"; if c do … ms = X
//!   end` becomes `ms = if c do … X else ms end`.
//!
//! Everything else is per-construct mapping: `is_a?`/`nil?`/`to_s`/
//! `length`/`gsub` Sends, `[]` slicing → `String.slice`, Regex/Range/
//! Hash literals, string interpolation (syntax matches Ruby).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::expr::{BoolOpKind, Expr, ExprNode, InterpPart, LValue, Literal};

thread_local! {
    /// Simple class name → emitted `V2.*` module name, for the unit
    /// currently being emitted. Elixir doesn't resolve a bare sibling
    /// reference (`MatchResult` inside `V2.ActionDispatch.Router`) the
    /// way Ruby's lexical scoping does, so `emit_const` rewrites such
    /// refs to the fully-qualified module name using this map.
    static MODULE_NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Register the `V2.*` names of the classes in the current unit (clears
/// any prior registration — scope is one runtime file). Called from the
/// elixir2 overlay's transform before the unit is emitted.
pub(super) fn register_modules<'a>(classes: impl IntoIterator<Item = &'a crate::dialect::LibraryClass>) {
    MODULE_NAMES.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        for c in classes {
            let full = super::library::v2_module_name(c.name.0.as_str());
            let simple = c
                .name
                .0
                .as_str()
                .rsplit("::")
                .next()
                .unwrap_or_else(|| c.name.0.as_str())
                .to_string();
            m.insert(simple, full);
        }
    });
}

/// Emit a method body as Elixir (indent level 0; the caller indents).
pub(super) fn emit_method_body(body: &Expr) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => emit_stmts(exprs),
        _ => emit_tail(body),
    }
}

/// Render an expression for use as a top-level module constant value
/// (the `value` in `@name value`). `.freeze` is stripped (Elixir is
/// immutable). Used by `library::format_constant`.
pub(super) fn emit_const_value(value: &Expr) -> String {
    emit_expr(value)
}

// ---- statement-list emit (return-elim + cond-rebind) ----------------

fn emit_stmts(stmts: &[Expr]) -> String {
    let Some((head, rest)) = stmts.split_first() else {
        return "nil".to_string();
    };
    if rest.is_empty() {
        return emit_tail(head);
    }

    // Guard clause: `return X if cond` → `if cond do X else <rest> end`.
    if let ExprNode::If { cond, then_branch, else_branch } = &*head.node {
        if is_empty(else_branch) && ends_in_return(then_branch) {
            let cond_s = emit_expr(cond);
            let then_s = emit_return_value(then_branch);
            let else_s = emit_stmts(rest);
            return format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            );
        }

        // Conditional reassignment: an `if`/`elsif` chain where every
        // branch yields the same reassigned local `v` (reassigns it, is
        // empty, or is a nested chain yielding it). Elixir scoping
        // discards a rebind inside a branch, so lift the whole chain to
        // `v = if cond do <new> else <…> end`. Covers the single `if`
        // (`@x = v unless c`) and `if/elsif` chains (`[]=`/case-on-key).
        if let Some(v) = chain_reassigned_var(then_branch, else_branch) {
            let lifted = format!("{v} = {}", render_chain(cond, then_branch, else_branch, &v));
            return format!("{lifted}\n{}", emit_stmts(rest));
        }
    }

    format!("{}\n{}", emit_stmt(head), emit_stmts(rest))
}

/// A single non-terminal statement (a `let` binding or a bare expr).
fn emit_stmt(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
        | ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            // Elixir has no instance state; an `@ivar =` rebind at this
            // depth is a plain local rebind (the real mutation-threading
            // work lands in a later phase).
            format!("{} = {}", name, emit_expr(value))
        }
        _ => emit_expr(e),
    }
}

/// The trailing (value-producing) position of a block. A bare `return X`
/// here is just `X`.
fn emit_tail(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Return { value } => emit_expr(value),
        ExprNode::Seq { exprs } if !exprs.is_empty() => emit_stmts(exprs),
        _ => emit_expr(e),
    }
}

/// True when `e` is an empty/absent branch (modifier-`if` has no else).
fn is_empty(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
        || matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

/// A local `v` reassigned across an `if`/`elsif` chain where *every*
/// branch yields it — the signal to lift the chain to `v = if … end`.
fn chain_reassigned_var(then_branch: &Expr, else_branch: &Expr) -> Option<String> {
    let v = reassigned_var(then_branch).or_else(|| reassigned_var(else_branch))?;
    (branch_yields(then_branch, v) && branch_yields(else_branch, v)).then(|| v.to_string())
}

/// A branch "yields `v`" if it leaves `v` as its value: empty (unchanged),
/// a trailing `v = …` rebind, or a nested chain whose branches all yield.
fn branch_yields(b: &Expr, v: &str) -> bool {
    if is_empty(b) {
        return true;
    }
    match &*b.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => name.as_str() == v,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(|l| branch_yields(l, v)),
        ExprNode::If { then_branch, else_branch, .. } => {
            branch_yields(then_branch, v) && branch_yields(else_branch, v)
        }
        _ => false,
    }
}

/// Render a chain as `if cond do <yields v> else <yields v> end`, where
/// each branch produces `v`'s next value (unchanged `v` for empty, the
/// rebind's RHS for a `v = …`, a nested chain recursively).
fn render_chain(cond: &Expr, then_branch: &Expr, else_branch: &Expr, v: &str) -> String {
    format!(
        "if {} do\n{}\nelse\n{}\nend",
        emit_expr(cond),
        indent(&branch_render(then_branch, v), 1),
        indent(&branch_render(else_branch, v), 1),
    )
}

fn branch_render(b: &Expr, v: &str) -> String {
    if is_empty(b) {
        return v.to_string();
    }
    match &*b.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            render_chain(cond, then_branch, else_branch, v)
        }
        _ => emit_block_with_value(b),
    }
}

/// True when the then-branch of a guard ends in a `return`.
fn ends_in_return(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Return { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(ends_in_return),
        _ => false,
    }
}

/// Emit the value a guard's `return` yields.
fn emit_return_value(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Return { value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            // Lets before the return stay; the trailing return becomes
            // the block's value.
            emit_stmts(exprs)
        }
        _ => emit_expr(e),
    }
}

/// If the block's last statement reassigns an (already-bound) local
/// variable, return its name — the signal for the cond-rebind lift.
fn reassigned_var(e: &Expr) -> Option<&str> {
    let last = match &*e.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => e,
    };
    match &*last.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => Some(name.as_str()),
        _ => None,
    }
}

/// Emit a block whose trailing `v = X` is rewritten to yield `X`
/// (used inside a cond-rebind `if`). Leading lets are preserved.
fn emit_block_with_value(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let (last, leading) = exprs.split_last().unwrap();
            let mut lines: Vec<String> = leading.iter().map(emit_stmt).collect();
            match &*last.node {
                ExprNode::Assign { value, .. } => lines.push(emit_expr(value)),
                _ => lines.push(emit_stmt(last)),
            }
            lines.join("\n")
        }
        ExprNode::Assign { value, .. } => emit_expr(value),
        _ => emit_tail(e),
    }
}

// ---- expression emit ------------------------------------------------

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => emit_const(path),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Send { recv, method, args, block, .. } => match block {
            Some(blk) => emit_block_call(recv.as_ref(), method.as_str(), args, blk),
            None => emit_send(recv.as_ref(), method.as_str(), args),
        },
        ExprNode::Return { value } => emit_expr(value),
        ExprNode::Raise { value } => format!("raise {}", emit_expr(value)),
        // `yield a, b` → call the block passed as the trailing `block_fn`
        // param (added by emit_fn when the body yields).
        ExprNode::Yield { args } => format!("block_fn.({})", emit_args(args)),
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => emit_stmts(exprs),
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = emit_tail(then_branch);
            let else_s = if is_empty(else_branch) {
                "nil".to_string()
            } else {
                emit_tail(else_branch)
            };
            format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_expr(v))
                    } else {
                        format!("{} => {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        ExprNode::Range { begin, end, exclusive } => {
            let b = begin.as_ref().map(emit_expr).unwrap_or_default();
            let e = end.as_ref().map(emit_expr).unwrap_or_default();
            // Elixir ranges are inclusive `b..e`; exclusive Ruby `b...e`
            // → `b..(e - 1)//1` is awkward, so use `Range` only for the
            // inclusive/endless forms json_builder needs (`20..`).
            if *exclusive {
                format!("{b}..{e}//1")
            } else {
                format!("{b}..{e}")
            }
        }
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::Cast { value, .. } => emit_expr(value),
        other => crate::emit::diagnostics::report_unsupported("elixir2", other.kind_str(), ""),
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // A `self.foo(...)` call inside a module is just a same-module
    // bareword call in Elixir — collapse the receiver.
    let recv = match recv {
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => None,
        other => other,
    };

    // `recv.__index_put__(k, v)` (from local_accumulation's `x[k]=v`)
    // rendered by receiver type: a struct routes to its `put` setter,
    // a map (or unknown) to `Map.put`.
    if method == "__index_put__" && args.len() == 2 {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            let (k, v) = (emit_expr(&args[0]), emit_expr(&args[1]));
            if let Some(crate::ty::Ty::Class { id, .. }) = r.ty.as_ref() {
                let module = super::library::v2_module_name(id.0.as_str());
                return format!("{module}.put({r_s}, {k}, {v})");
            }
            return format!("Map.put({r_s}, {k}, {v})");
        }
    }

    // `record.__field__(:x)` → `record.x` (struct field read).
    if method == "__field__" && args.len() == 1 {
        if let Some(r) = recv {
            let field = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                _ => emit_expr(&args[0]),
            };
            return format!("{}.{field}", emit_expr(r));
        }
    }

    // `record.__struct_put__(:field, value)` → `%{record | field: value}`
    // (the mutation-threading bridge — see lower::functionalize::
    // mutation_to_struct_return).
    if method == "__struct_put__" && args.len() == 2 {
        if let Some(r) = recv {
            let field = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                _ => emit_expr(&args[0]),
            };
            return format!("%{{{} | {field}: {}}}", emit_expr(r), emit_expr(&args[1]));
        }
    }

    // Unary `!x` (Ruby `CallNode` with method `!`, no args) → Elixir
    // `!(x)`. Parenthesized so it binds tighter than any operator in the
    // receiver (`!(a and b)`, `!(is_nil(x))`).
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!({})", emit_expr(r));
        }
    }

    // `.freeze` — Elixir is immutable; the receiver is the value.
    if method == "freeze" && args.is_empty() {
        if let Some(r) = recv {
            return emit_expr(r);
        }
    }

    // `recv.nil?` → `is_nil(recv)`.
    if method == "nil?" && args.is_empty() {
        if let Some(r) = recv {
            return format!("is_nil({})", emit_expr(r));
        }
    }

    // `recv.is_a?(Class)` → Elixir type guard / equality.
    if method == "is_a?" && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            if let Some(class) = path.last() {
                let r_s = emit_expr(r);
                let mapped = match class.as_str() {
                    "TrueClass" => Some(format!("{r_s} == true")),
                    "FalseClass" => Some(format!("{r_s} == false")),
                    "NilClass" => Some(format!("is_nil({r_s})")),
                    "Integer" => Some(format!("is_integer({r_s})")),
                    "Float" => Some(format!("is_float({r_s})")),
                    "Numeric" => Some(format!("is_number({r_s})")),
                    "String" => Some(format!("is_binary({r_s})")),
                    "Array" => Some(format!("is_list({r_s})")),
                    "Hash" => Some(format!("is_map({r_s})")),
                    _ => None,
                };
                if let Some(s) = mapped {
                    return s;
                }
            }
        }
    }

    // `s.gsub(regex, hash)` → `Regex.replace(regex, s, fn m -> Map.get(hash, m) end)`.
    // `s.gsub(needle, repl)` (string args) → `String.replace(s, needle, repl)`.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            let a0 = emit_expr(&args[0]);
            let a1 = emit_expr(&args[1]);
            let hash_repl = matches!(
                &*args[1].node,
                ExprNode::Hash { .. } | ExprNode::Const { .. }
            );
            if hash_repl {
                return format!(
                    "Regex.replace({a0}, {r_s}, fn m -> Map.get({a1}, m, \"\") end)"
                );
            }
            return format!("String.replace({r_s}, {a0}, {a1})");
        }
    }

    // `recv.to_s` → `to_string(recv)`.
    if method == "to_s" && args.is_empty() {
        if let Some(r) = recv {
            return format!("to_string({})", emit_expr(r));
        }
    }

    // `recv.length` / `recv.size` — lists use `Kernel.length/1`, strings
    // use `String.length/1`. Driven by the analyzer's `Ty` on the
    // receiver; defaults to `String.length` when the type is unknown.
    if (method == "length" || method == "size") && args.is_empty() {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            return if recv_is_array(r) {
                format!("length({r_s})")
            } else if recv_is_hash(r) {
                format!("map_size({r_s})")
            } else {
                format!("String.length({r_s})")
            };
        }
    }

    // Ruby Hash methods → Elixir `Map.*` (gated on a Hash-typed receiver,
    // so a struct's `key?`/`keys` route to its own methods instead).
    if let Some(r) = recv {
        if recv_is_hash(r) {
            let r_s = emit_expr(r);
            match (method, args.len()) {
                ("keys", 0) => return format!("Map.keys({r_s})"),
                ("values", 0) => return format!("Map.values({r_s})"),
                ("empty?", 0) => return format!("map_size({r_s}) == 0"),
                ("key?" | "has_key?" | "include?", 1) => {
                    return format!("Map.has_key?({r_s}, {})", emit_expr(&args[0]))
                }
                ("fetch", 2) => {
                    return format!("Map.get({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]))
                }
                ("fetch", 1) => return format!("Map.fetch!({r_s}, {})", emit_expr(&args[0])),
                _ => {}
            }
        }
    }

    // `self[k]` / `self[k] = v` on the threaded `record` → the renamed
    // same-module accessor (`def []` → `get`, `def []=` → `put`).
    if (method == "[]" || method == "[]=") && recv.is_some_and(is_record_var) {
        let fname = super::library::elixir_fn_name(method);
        return format!("{fname}(record, {})", emit_args(args));
    }
    // `self.foo(args)` / `self.foo` on the threaded `record` is a method
    // call (field reads come through `__field__`, handled above) → the
    // same-module `foo(record, …)`, including 0-arg `self.to_h`.
    if recv.is_some_and(is_record_var)
        && method.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        let fname = super::library::elixir_fn_name(method);
        return if args.is_empty() {
            format!("{fname}(record)")
        } else {
            format!("{fname}(record, {})", emit_args(args))
        };
    }

    // `recv[...]` indexing.
    if method == "[]" && recv.is_some() {
        let r = recv.unwrap();
        let r_s = emit_expr(r);
        // List indexing: `list[i]` raises in Elixir (lists aren't Access
        // by integer), so route through `Enum`.
        if recv_is_array(r) {
            if args.len() == 1 {
                if let ExprNode::Range { .. } = &*args[0].node {
                    return format!("Enum.slice({r_s}, {})", emit_expr(&args[0]));
                }
                return format!("Enum.at({r_s}, {})", emit_expr(&args[0]));
            }
            if args.len() == 2 {
                return format!("Enum.slice({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]));
            }
        }
        // Two-arg `recv[start, len]` → string slice.
        if args.len() == 2 {
            return format!("String.slice({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]));
        }
        if args.len() == 1 {
            // Range index → string slice. Elixir has no endless-range
            // literal (`20..` is a syntax error), so an open end uses
            // the `start, length` form; bounded ranges slice directly.
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                let b = begin.as_ref().map(emit_expr).unwrap_or_else(|| "0".into());
                return match end {
                    None => format!("String.slice({r_s}, {b}, String.length({r_s}))"),
                    Some(e) if *exclusive => {
                        format!("String.slice({r_s}, {b}..({} - 1)//1)", emit_expr(e))
                    }
                    Some(e) => format!("String.slice({r_s}, {b}..{})", emit_expr(e)),
                };
            }
            // Otherwise a map/keyword access: `map[key]`.
            return format!("{r_s}[{}]", emit_expr(&args[0]));
        }
        return format!("{r_s}[{}]", emit_args(args));
    }

    // Binary operators ride the Send channel. Comparisons map 1:1;
    // `+`/`-`/`*`/`/`/`%`/`**` dispatch on operand type because Elixir's
    // arithmetic operators are numeric-only (strings use `<>`, lists
    // `++`/`--`, etc.). Ported from the legacy elixir emitter.
    if let (Some(r), [arg]) = (recv, args) {
        // `== nil` / `!= nil` → `is_nil/1` guard.
        if method == "==" || method == "!=" {
            use crate::emit::shared::eq::{classify_eq, EqCase};
            if let EqCase::NilCheck { subject } = classify_eq(r, arg) {
                let s = emit_expr(subject);
                return if method == "==" {
                    format!("is_nil({s})")
                } else {
                    format!("not is_nil({s})")
                };
            }
        }
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_add(r, arg) {
                AddCase::StringConcat => format!("{ls} <> {rs}"),
                AddCase::ArrayConcat { .. } => format!("{ls} ++ {rs}"),
                AddCase::Incompatible => {
                    r#"raise "roundhouse: + with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} + {rs}"),
            };
        }
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => format!("{ls} -- {rs}"),
                SubCase::Incompatible => {
                    r#"raise "roundhouse: - with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} - {rs}"),
            };
        }
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_mul(r, arg) {
                MulCase::StringRepeat => format!("String.duplicate({ls}, {rs})"),
                MulCase::ArrayRepeat { .. } => format!("List.duplicate({ls}, {rs}) |> List.flatten()"),
                MulCase::ArrayJoin { .. } => format!("Enum.join({ls}, {rs})"),
                MulCase::Incompatible => {
                    r#"raise "roundhouse: * with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} * {rs}"),
            };
        }
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_modulo(r, arg) {
                ModuloCase::NumericPromote => format!(":math.fmod({ls}, {rs})"),
                ModuloCase::StringFormat => {
                    r#"raise "roundhouse: String % (sprintf) not yet supported for Elixir target""#.to_string()
                }
                ModuloCase::Incompatible => {
                    r#"raise "roundhouse: % with incompatible operand types""#.to_string()
                }
                // Numeric/Unknown: rem/2 (integer) or fmod (float recv).
                _ if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Float)) => {
                    format!(":math.fmod({ls}, {rs})")
                }
                _ => format!("rem({ls}, {rs})"),
            };
        }
        if method == "**" {
            return format!(":math.pow({}, {})", emit_expr(r), emit_expr(arg));
        }
        if is_infix(method) {
            return format!("{} {method} {}", emit_expr(r), emit_expr(arg));
        }
    }

    // Ruby String / Hash methods → Elixir module-function calls (Elixir
    // has no `recv.method` dispatch on builtins).
    if let Some(r) = recv {
        let r_s = emit_expr(r);
        match (method, args.len()) {
            ("upcase", 0) => return format!("String.upcase({r_s})"),
            ("downcase", 0) => return format!("String.downcase({r_s})"),
            ("strip", 0) => return format!("String.trim({r_s})"),
            ("split", 1) => return format!("String.split({r_s}, {})", emit_expr(&args[0])),
            ("start_with?", 1) => {
                return format!("String.starts_with?({r_s}, {})", emit_expr(&args[0]))
            }
            ("end_with?", 1) => {
                return format!("String.ends_with?({r_s}, {})", emit_expr(&args[0]))
            }
            // `acc.merge({k => v})` — the threaded-accumulator update
            // emitted by while_to_recursion.
            ("merge", 1) => return format!("Map.merge({r_s}, {})", emit_expr(&args[0])),
            _ => {}
        }
    }

    // Default call forms.
    match recv {
        None => {
            // Bareword — a function in the enclosing module (e.g.
            // `encode_string(v)`).
            if args.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, emit_args(args))
            }
        }
        Some(r) => {
            let r_s = emit_expr(r);
            if args.is_empty() {
                format!("{r_s}.{method}")
            } else {
                format!("{r_s}.{method}({})", emit_args(args))
            }
        }
    }
}

fn emit_args(args: &[Expr]) -> String {
    args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
}

/// `recv.each do |x| body end` → an `Enum.*` call with the block as an
/// anonymous function.
///
/// If the block reassigns a single outer local (an accumulator, e.g.
/// `result = result.merge(...)`), it lowers to `Enum.reduce`, threading
/// that local as the accumulator and rebinding it at the call site —
/// the Elixir answer to block-local mutation not leaking. A
/// non-accumulating block uses the directly-mapped `Enum.*` (each/map/
/// filter/…). Multi-accumulator blocks (tuple reduce) aren't covered.
fn emit_block_call(recv: Option<&Expr>, method: &str, args: &[Expr], block: &Expr) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return crate::emit::diagnostics::report_unsupported(
            "elixir2",
            "block",
            &format!("non-lambda block on `{method}`"),
        );
    };
    let recv_s = recv.map(emit_expr).unwrap_or_default();
    let body_s = emit_method_body(body);
    let block_params = params.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    // The `Enum.*` callback receives ONE element per item; a Ruby block
    // with multiple params (`each do |k, v|`) is iterating pairs, so the
    // element destructures as a tuple `{k, v}`.
    let element = match block_params.len() {
        0 => "_".to_string(),
        1 => block_params[0].clone(),
        _ => format!("{{{}}}", block_params.join(", ")),
    };

    let accs = block_accumulators(body, params);
    if accs.len() > 1 {
        return crate::emit::diagnostics::report_unsupported(
            "elixir2",
            "block",
            "block reassigns multiple outer locals (tuple reduce)",
        );
    }
    if let Some(acc) = accs.first() {
        // Accumulating block → reduce. The fn takes the block params plus
        // the accumulator; the body rebinds the acc and yields it (we
        // append a trailing `acc` reference so the rebind is preserved as
        // a statement rather than collapsing to its value). The outer
        // rebind captures the fold's result.
        let fn_params = format!("{element}, {acc}");
        let acc_var = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
            id: crate::ident::VarId(0),
            name: crate::ident::Symbol::from(acc.as_str()),
        });
        let mut stmts: Vec<Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![body.clone()],
        };
        stmts.push(acc_var);
        let threaded = emit_method_body(&Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Seq { exprs: stmts },
        ));
        return format!(
            "{acc} = Enum.reduce({recv_s}, {acc}, fn {fn_params} ->\n{}\nend)",
            indent(&threaded, 1),
        );
    }

    // Non-accumulating: directly-mapped Enum.* with the block as a fn.
    let enum_fn = enum_method(method);
    let lead = if args.is_empty() {
        String::new()
    } else {
        format!("{}, ", emit_args(args))
    };
    format!(
        "Enum.{enum_fn}({recv_s}, {lead}fn {element} ->\n{}\nend)",
        indent(&body_s, 1),
    )
}

/// Outer locals the block reassigns to a value derived from themselves
/// (`v = …v…`) — i.e. accumulators threaded through a fold. Block
/// params and block-local lets (`tmp = x`) are excluded. Detection is
/// over the block body's top-level statements (renders the RHS and
/// checks for a self-reference token).
fn block_accumulators(body: &Expr, params: &[crate::ident::Symbol]) -> Vec<String> {
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => std::slice::from_ref(body),
    };
    let mut out: Vec<String> = Vec::new();
    for s in stmts {
        if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*s.node {
            let n = name.as_str();
            if !params.iter().any(|p| p.as_str() == n)
                && super::library::references_token(&emit_expr(value), n)
                && !out.iter().any(|a| a == n)
            {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// Map a Ruby Enumerable method to its `Enum` counterpart. Renamed
/// forms are listed; everything else passes through (most names match).
fn enum_method(method: &str) -> &str {
    match method {
        "collect" => "map",
        "select" | "find_all" => "filter",
        "detect" => "find",
        "inject" => "reduce",
        other => other,
    }
}

/// True when the analyzer typed `e` as an `Array` — the signal to use
/// `Enum`/`Kernel.length` rather than the `String`/`Access` forms.
fn recv_is_array(e: &Expr) -> bool {
    matches!(e.ty.as_ref(), Some(crate::ty::Ty::Array { .. }))
}

/// True when `e` is the threaded `record` var (self) — its `[]`/`[]=`
/// route to the same-module renamed accessor.
fn is_record_var(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "record")
}

/// True when the analyzer typed `e` as a `Hash` — route its methods to `Map.*`.
fn recv_is_hash(e: &Expr) -> bool {
    matches!(e.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
}

/// Ruby infix operators whose Elixir spelling is identical.
/// Operators whose Elixir spelling is a plain infix (comparisons, plus
/// `/` which is numeric-only in both languages). `+`/`-`/`*`/`%`/`**`
/// are handled by the type-dispatching arms above.
fn is_infix(method: &str) -> bool {
    matches!(
        method,
        // `++`/`--` are generated by local_accumulation (list append /
        // difference); the rest are comparisons + numeric `/`.
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "/" | "and" | "or" | "++" | "--"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{Symbol, VarId};
    use crate::ty::Ty;

    fn var_t(name: &str, ty: Ty) -> Expr {
        let mut e = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(name),
        });
        e.ty = Some(ty);
        e
    }
    fn binop(l: Expr, op: &str, r: Expr) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(l),
            method: Symbol::from(op),
            args: vec![r],
            block: None,
            parenthesized: false,
        })
    }
    fn arr() -> Ty {
        Ty::Array { elem: Box::new(Ty::Untyped) }
    }

    fn unary(method: &str, recv: Expr) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    }

    fn block_send(recv: &str, method: &str, params: &[&str], body: Expr) -> Expr {
        let lambda = Expr::new(crate::span::Span::synthetic(), ExprNode::Lambda {
            params: params.iter().map(|p| Symbol::from(*p)).collect(),
            block_param: None,
            body,
            block_style: Default::default(),
        });
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(var_t(recv, Ty::Untyped)),
            method: Symbol::from(method),
            args: vec![],
            block: Some(lambda),
            parenthesized: false,
        })
    }

    #[test]
    fn block_calls_map_to_enum() {
        let each = block_send("items", "each", &["x"], var_t("x", Ty::Untyped));
        assert_eq!(emit_expr(&each), "Enum.each(items, fn x ->\n  x\nend)");
        // Renames: collect→map, select→filter, detect→find.
        let mapped = block_send("items", "collect", &["x"], var_t("x", Ty::Untyped));
        assert!(emit_expr(&mapped).starts_with("Enum.map(items, fn x ->"));
        let filtered = block_send("items", "select", &["x"], var_t("x", Ty::Untyped));
        assert!(emit_expr(&filtered).starts_with("Enum.filter(items, fn x ->"));
        // Two-param block (`each do |k, v|`) destructures the element tuple.
        let kv = block_send("h", "each", &["k", "v"], var_t("k", Ty::Untyped));
        assert!(emit_expr(&kv).starts_with("Enum.each(h, fn {k, v} ->"));
    }

    #[test]
    fn accumulating_block_lowers_to_reduce() {
        // items.each do |x| total = total + x end
        let acc_assign = Expr::new(crate::span::Span::synthetic(), ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("total") },
            value: binop(var_t("total", Ty::Int), "+", var_t("x", Ty::Int)),
        });
        let e = block_send("items", "each", &["x"], acc_assign);
        let out = emit_expr(&e);
        assert_eq!(
            out,
            "total = Enum.reduce(items, total, fn x, total ->\n  total = total + x\n  total\nend)",
            "got: {out}"
        );
    }

    #[test]
    fn hash_methods_map_to_elixir_map() {
        let hash = || {
            var_t("h", Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) })
        };
        let no_args = |m: &str| Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(hash()),
            method: Symbol::from(m),
            args: vec![],
            block: None,
            parenthesized: false,
        });
        assert_eq!(emit_expr(&no_args("keys")), "Map.keys(h)");
        assert_eq!(emit_expr(&no_args("values")), "Map.values(h)");
        assert_eq!(emit_expr(&no_args("length")), "map_size(h)");
        assert_eq!(emit_expr(&no_args("empty?")), "map_size(h) == 0");
        // key?(k)
        let key_q = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(hash()),
            method: Symbol::from("key?"),
            args: vec![var_t("k", Ty::Str)],
            block: None,
            parenthesized: false,
        });
        assert_eq!(emit_expr(&key_q), "Map.has_key?(h, k)");
    }

    #[test]
    fn unary_bang_negates() {
        // `!(@notice.nil?)` shape: `!` over a `nil?` call.
        let inner = unary("nil?", var_t("x", Ty::Str));
        assert_eq!(emit_expr(&inner), "is_nil(x)");
        assert_eq!(emit_expr(&unary("!", inner)), "!(is_nil(x))");
    }

    #[test]
    fn operator_dispatch_by_operand_type() {
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Str), "+", var_t("b", Ty::Str))), "a <> b");
        assert_eq!(emit_expr(&binop(var_t("a", arr()), "+", var_t("b", arr()))), "a ++ b");
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Int), "+", var_t("b", Ty::Int))), "a + b");
        assert_eq!(emit_expr(&binop(var_t("a", arr()), "-", var_t("b", arr()))), "a -- b");
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Int), "<", var_t("b", Ty::Int))), "a < b");
    }
}

fn emit_const(path: &[crate::ident::Symbol]) -> String {
    // SCREAMING_SNAKE single-segment name → module attribute (`ESCAPES`
    // → `@escapes`). CamelCase → a module reference (dotted).
    if path.len() == 1 && is_screaming_snake(path[0].as_str()) {
        return format!("@{}", path[0].as_str().to_lowercase());
    }
    // A reference to a sibling module in the same unit — whether bare
    // (`MatchResult`) or fully qualified (`ActionDispatch::Router::
    // MatchResult`) — resolves by its last segment to the emitted
    // `V2.*` name.
    if let Some(last) = path.last() {
        if let Some(full) = MODULE_NAMES.with(|m| m.borrow().get(last.as_str()).cloned()) {
            return full;
        }
    }
    path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("\"");
    for p in parts {
        match p {
            InterpPart::Text { value } => push_escaped(&mut out, value),
            InterpPart::Expr { expr } => {
                out.push_str("#{");
                out.push_str(&emit_expr(expr));
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}

/// Escape a string for an Elixir double-quoted literal body. Handles
/// the quote/backslash/`#` (interpolation) cases plus the control
/// characters json_builder's `ESCAPES` keys carry as raw bytes
/// (backspace `\b` 0x08, form feed `\f` 0x0c, …). Other control chars
/// fall back to `\xHH`.
fn push_escaped(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            other => out.push(other),
        }
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => format!("{value:?}"),
        Literal::Str { value } => emit_str_literal(value),
        Literal::Sym { value } => format!(":{value}"),
        Literal::Regex { pattern, flags } => emit_regex(pattern, flags),
    }
}

fn emit_str_literal(value: &str) -> String {
    let mut out = String::from("\"");
    push_escaped(&mut out, value);
    out.push('"');
    out
}

/// Ruby `/pat/flags` → Elixir `~r/pat/flags`. `pattern` is the regex
/// source text (backslash escapes preserved). Ruby's `m` flag (dotall)
/// maps to Elixir's `s`; `i`/`x` carry over.
fn emit_regex(pattern: &str, flags: &str) -> String {
    let escaped = pattern.replace('/', "\\/");
    let ex_flags: String = flags
        .chars()
        .filter_map(|f| match f {
            'i' => Some('i'),
            'm' => Some('s'),
            'x' => Some('x'),
            _ => None,
        })
        .collect();
    format!("~r/{escaped}/{ex_flags}")
}

fn is_screaming_snake(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.chars().any(|c| c.is_ascii_uppercase())
}

/// Indent every non-empty line by `levels * 2` spaces.
pub(super) fn indent(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
