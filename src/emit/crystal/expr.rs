//! Expression emission: per-AST-node converter for Crystal.
//!
//! Mirrors `src/emit/ruby/expr.rs` because Crystal's surface syntax
//! is Ruby-flavored (def/end, do |x|, string interp, blocks). The
//! divergences are localized:
//!   - `Lambda` needs typed params (Crystal Procs are typed); emit
//!     a closure form or fall back to a stub when types are missing.
//!   - Hash literals: emit the same shorthand (`key: val`) Spinel
//!     emits — Crystal accepts that as NamedTuple syntax, which works
//!     when helpers take `**opts`.
//!   - `Pattern` / `Case in` Ruby-3-style pattern matching maps to
//!     Crystal's narrower `case when` semantics; we reuse `case when`
//!     for the simple shapes the lowerer produces today.

use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
use crate::ident::Symbol;

use super::shared::{escape_ident, indent_lines};

pub fn emit_expr(e: &Expr) -> String {
    let raw = emit_node(&e.node);
    // Crystal's strict-typing flow analysis flushes ivar narrowing on
    // any intervening method call. Even after `@article = Article.find(...)`,
    // the next `Comment.from_params(...)` resets `@article` to its
    // declared `Article?` shape. The body-typer's Seq walk threads
    // ivar bindings forward, so by the time we reach a downstream
    // `@article` read, `e.ty` is the narrowed non-nilable type.
    // Emit `@article.not_nil!` at those reads to bridge the gap —
    // safe (the typer's narrowing is sound) and idiomatic Crystal.
    //
    // Applied to Ivar reads typed as a concrete non-nilable type
    // (`Class`, primitives `Int`/`Str`/`Bool`/`Float`/`Sym`,
    // collections `Array`/`Hash`). Excluded: `Ty::Untyped`,
    // `Ty::Nil`, `Ty::Var`, `Ty::Union` — these either are or
    // already include nil; `.not_nil!` would be wrong or redundant.
    // Schema-derived attr accessors carry the non-nilable column
    // type (e.g. `Ty::Int` for `article_id` even when the underlying
    // `property` declaration is `Int64?`) — narrowing here matches
    // what the body-typer guarantees.
    if let ExprNode::Ivar { .. } = &*e.node {
        if let Some(ty) = e.ty.as_ref() {
            if is_non_nilable_concrete(ty) {
                return format!("{raw}.not_nil!");
            }
        }
    }
    // Same bridge for `recv.attr` Send dispatches that resolve to a
    // model column reader. Crystal's auto-generated `property name : T?`
    // getter returns the nilable form, but the body-typer types the
    // accessor's result by its schema column type (non-nilable when
    // the column is `null: false`). Narrow at the call site for Sends
    // that look like attribute reads — zero-arg, no block, receiver
    // typed as a `Ty::Class` instance — to keep typed call sites
    // (e.g. `RouteHelpers.x_path(comment.article_id)` requiring
    // `Int64` not `Int64?`) compiling. Stricter than the Ivar rule —
    // the receiver-class check filters out unrelated zero-arg sends
    // (e.g. `1.to_s`, `"".size`) where `.not_nil!` would be wrong.
    if let ExprNode::Send { recv: Some(recv), args, block: None, .. } = &*e.node {
        if args.is_empty()
            && matches!(recv.ty.as_ref(), Some(crate::ty::Ty::Class { .. }))
            && matches!(e.ty.as_ref(), Some(t) if is_non_nilable_primitive(t))
        {
            return format!("{raw}.not_nil!");
        }
    }
    raw
}

/// Subset of [`is_non_nilable_concrete`] limited to primitive types
/// that frequently appear as nilable column properties in Crystal
/// (`Int`, `Str`, `Bool`, `Float`). Class types are excluded — they
/// hit the Ivar/instance path with their own narrowing.
fn is_non_nilable_primitive(ty: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym)
}

/// True for Ty variants that emit as non-nilable concrete Crystal types.
/// `Untyped`/`Nil`/`Var`/`Union`/`Bottom` excluded — they're either
/// already nilable or unknown, so `.not_nil!` would be incorrect.
fn is_non_nilable_concrete(ty: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    matches!(
        ty,
        Ty::Class { .. }
            | Ty::Int
            | Ty::Float
            | Ty::Bool
            | Ty::Str
            | Ty::Sym
            | Ty::Array { .. }
            | Ty::Hash { .. }
            | Ty::Tuple { .. }
    )
}

/// Public entry point used by `runtime_loader::crystal_units` for
/// module-level constant initializers (`HTML_ESCAPES = { ... }.freeze`
/// in view_helpers.rb, etc.). Same renderer; the function-typed
/// alias is a stable hook the loader plugs into `TargetEmit`.
pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

fn is_empty_branch(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// Map Ruby stdlib type names that don't exist in Crystal to their
/// Crystal analogs. `Integer` (abstract integer in Ruby) maps to
/// `Int` (abstract integer in Crystal — parent of Int8…Int128).
/// Used at Const emit; only fires for single-segment Consts.
fn rewrite_stdlib_const(name: &str) -> Option<&'static str> {
    match name {
        "Integer" => Some("Int"),
        _ => None,
    }
}

/// Resolve a bare framework class name to its fully-qualified Crystal
/// module path. The body-typer's class registry holds aliases under
/// the last-segment name (`ViewHelpers`, `Parameters`, ...), but
/// Crystal's namespace resolution needs the parent module spelled
/// out at every reference site that isn't itself nested inside the
/// home module. Returns `None` when the bare name isn't a known
/// framework class — caller falls back to the unqualified spelling.
fn qualify_bare_framework(name: &str) -> Option<&'static str> {
    match name {
        "HashWithIndifferentAccess" => Some("::ActiveSupport::HashWithIndifferentAccess"),
        "Parameters" => Some("::ActionController::Parameters"),
        "ParameterMissing" => Some("::ActionController::ParameterMissing"),
        "Router" => Some("::ActionDispatch::Router"),
        "ViewHelpers" => Some("::ActionView::ViewHelpers"),
        "FormBuilder" => Some("::ActionView::FormBuilder"),
        _ => None,
    }
}

fn emit_node(n: &ExprNode) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => escape_ident(name.as_str()),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::SelfRef => "self".to_string(),
        ExprNode::Const { path } => {
            // Top-level framework module references need an explicit
            // `::` prefix when the call site is INSIDE the same module
            // (Crystal looks up `ActiveRecord` nested-first and finds
            // nothing, then tries outer scope — but reports the failed
            // nested lookup as `ActiveRecord::ActiveRecord:Module`).
            // Bare references to framework modules outside their own
            // namespace also benefit from the `::` prefix as a no-op
            // safety. Limited to a known list of framework module
            // names; app-level Const refs (`Article`, `Comment`, etc.)
            // emit unprefixed.
            let joined = path
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join("::");
            let first = path.first().map(|s| s.as_str()).unwrap_or("");
            if matches!(
                first,
                "ActiveRecord" | "ActionController" | "ActionView" | "ActionDispatch" | "ActiveSupport"
            ) {
                return format!("::{joined}");
            }
            // Bare last-segment refs to framework classes whose home
            // module is elsewhere — `ViewHelpers` lives under
            // `ActionView`, `HashWithIndifferentAccess` under
            // `ActiveSupport`, etc. The body-typer registers an alias
            // under the bare name so dispatch resolves, but Crystal
            // namespace lookup needs the qualified path. Re-attach
            // when we see a single-segment Const matching one of
            // these known names.
            if path.len() == 1 {
                if let Some(qualified) = qualify_bare_framework(first) {
                    return qualified.to_string();
                }
                if let Some(replacement) = rewrite_stdlib_const(first) {
                    return replacement.to_string();
                }
            }
            joined
        }
        ExprNode::Hash { entries, kwargs } => emit_hash(entries, *kwargs),
        ExprNode::Array { elements, style } => emit_array(elements, style),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, surface, left, right } => {
            emit_bool_op(*op, *surface, left, right)
        }
        ExprNode::Let { name, value, body, .. } => {
            format!("{name} = {}\n{}", emit_expr(value), emit_expr(body))
        }
        ExprNode::Lambda { params, block_param, body, .. } => {
            // Crystal procs require typed params. The lowered IR
            // doesn't always carry param types here, so fall back to
            // a block-param closure form (`->{ body }`) when params
            // are empty. With params, emit untyped `->(p) { body }`
            // and rely on Crystal's inference / context.
            let mut ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
            if let Some(b) = block_param {
                ps.push(format!("&{b}"));
            }
            if ps.is_empty() {
                format!("-> {{ {} }}", emit_expr(body))
            } else {
                format!("->({}) {{ {} }}", ps.join(", "), emit_expr(body))
            }
        }
        ExprNode::Apply { fun, args, block } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            let base = format!("{}.call({})", emit_expr(fun), args_s.join(", "));
            if let Some(b) = block {
                format!("{base} {{ {} }}", emit_expr(b))
            } else {
                base
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            // `require "x"` calls in Ruby method bodies are loadlate
            // imports — Ruby allows them anywhere, Crystal rejects
            // them outside file scope. Skip the call entirely; the
            // emitted Crystal file's top-level `require` statements
            // (or stdlib auto-load for Base64/JSON) handle the
            // semantic. Emits a comment so the diff stays auditable.
            if recv.is_none()
                && method.as_str() == "require"
                && args.len() == 1
                && matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Str { .. } }
                )
            {
                return format!("# Crystal: {} (skipped — module load handled at file scope)", emit_send_base(recv.as_ref(), method, args, *parenthesized));
            }
            // Buffer-accumulate idiom: `io << x` (Ruby) where `io` is a
            // String-typed local appends in place. Crystal Strings are
            // immutable and don't define `<<`; rewrite to the
            // assign-back form `io = io + x` (the lowerer's view-body
            // accumulator is the canonical case — `io = String.new`
            // followed by `io << helper(...)`). The bare `String.new`
            // initializer is rewritten to `""` below in emit_send_base.
            if method.as_str() == "<<" && args.len() == 1 {
                if let Some(r) = recv {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        if matches!(r.ty, Some(crate::ty::Ty::Str)) {
                            let var = escape_ident(name.as_str());
                            let val = emit_expr(&args[0]);
                            return format!("{var} = {var} + {val}");
                        }
                    }
                }
            }
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            match block {
                None => base,
                Some(b) => emit_do_block(&base, b),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_empty = is_empty_branch(else_branch);
            if else_empty
                && !matches!(&*then_branch.node, ExprNode::Seq { .. })
                && !then_s.contains('\n')
            {
                format!("{then_s} if {cond_s}")
            } else if else_empty {
                format!("if {cond_s}\n{}\nend", indent_lines(&then_s, 1))
            } else {
                format!(
                    "if {cond_s}\n{}\nelse\n{}\nend",
                    indent_lines(&then_s, 1),
                    indent_lines(&emit_expr(else_branch), 1),
                )
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            let mut s = format!("case {}\n", emit_expr(scrutinee));
            for arm in arms {
                s.push_str(&emit_arm(arm));
            }
            s.push_str("end");
            s
        }
        ExprNode::Seq { exprs } => {
            let mut out = String::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    if e.leading_blank_line {
                        out.push('\n');
                    }
                }
                out.push_str(&emit_expr(e));
            }
            out
        }
        ExprNode::Assign { target, value } => {
            format!("{} = {}", emit_lvalue(target), emit_expr(value))
        }
        ExprNode::Yield { args } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            if args_s.is_empty() {
                "yield".to_string()
            } else {
                format!("yield {}", args_s.join(", "))
            }
        }
        ExprNode::Raise { value } => emit_raise(value),
        ExprNode::RescueModifier { expr, fallback } => {
            format!("{} rescue {}", emit_expr(expr), emit_expr(fallback))
        }
        ExprNode::Return { value } => {
            if matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil }) {
                "return".to_string()
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::Super { args } => match args {
            None => "super".to_string(),
            Some(args) => {
                let args_s: Vec<String> = args.iter().map(emit_expr).collect();
                format!("super({})", args_s.join(", "))
            }
        },
        ExprNode::Next { value } => match value {
            None => "next".to_string(),
            Some(v) => format!("next {}", emit_expr(v)),
        },
        ExprNode::MultiAssign { targets, value } => {
            let lhs: Vec<String> = targets.iter().map(emit_lvalue).collect();
            format!("{} = {}", lhs.join(", "), emit_expr(value))
        }
        ExprNode::While { cond, body, until_form } => {
            let kw = if *until_form { "until" } else { "while" };
            format!(
                "{kw} {}\n{}\nend",
                emit_expr(cond),
                indent_lines(&emit_expr(body), 1),
            )
        }
        ExprNode::Range { begin, end, exclusive } => {
            let op = if *exclusive { "..." } else { ".." };
            let b = begin.as_ref().map(emit_expr).unwrap_or_default();
            let e = end.as_ref().map(emit_expr).unwrap_or_default();
            format!("{b}{op}{e}")
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            let mut s = String::new();
            if !*implicit {
                s.push_str("begin\n");
            }
            s.push_str(&indent_lines(&emit_expr(body), 1));
            s.push('\n');
            for rc in rescues {
                s.push_str("rescue");
                if !rc.classes.is_empty() {
                    let cs: Vec<String> = rc.classes.iter().map(emit_expr).collect();
                    s.push(' ');
                    s.push_str(&cs.join(", "));
                }
                if let Some(name) = &rc.binding {
                    s.push_str(&format!(" : {}", cs_or_exception(rc)));
                    s.push_str(&format!(" => {name}"));
                }
                s.push('\n');
                s.push_str(&indent_lines(&emit_expr(&rc.body), 1));
                s.push('\n');
            }
            if let Some(eb) = else_branch {
                s.push_str("else\n");
                s.push_str(&indent_lines(&emit_expr(eb), 1));
                s.push('\n');
            }
            if let Some(en) = ensure {
                s.push_str("ensure\n");
                s.push_str(&indent_lines(&emit_expr(en), 1));
                s.push('\n');
            }
            if !*implicit {
                s.push_str("end");
            }
            s
        }
        ExprNode::Cast { value, target_ty } => {
            // Crystal `.as(T)` — runtime-checked downcast. Wraps the
            // value in parens so chained casts and operator-precedence
            // edges (`a + b.as(Int64)`) parse correctly.
            format!("({}).as({})", emit_expr(value), super::ty::crystal_ty(target_ty))
        }
    }
}

/// In Crystal, `rescue ex` requires an exception class type when a
/// binding name is used (`rescue ex : Exception`). Helper to render
/// the type clause; falls back to `Exception` when none was named.
fn cs_or_exception(_rc: &crate::expr::RescueClause) -> String {
    "Exception".to_string()
}

fn emit_bool_op(
    op: crate::expr::BoolOpKind,
    _surface: crate::expr::BoolOpSurface,
    left: &Expr,
    right: &Expr,
) -> String {
    use crate::expr::BoolOpKind;
    // Crystal supports both `||`/`&&` and `or`/`and` keywords; the
    // symbol form is the unambiguous choice (Crystal's `or`/`and`
    // have different precedence than Ruby's, so symbol-form keeps
    // semantics unchanged).
    let op_s = match op {
        BoolOpKind::Or => "||",
        BoolOpKind::And => "&&",
    };
    format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::with_capacity(2);
    out.push('"');
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        '#' => out.push_str("\\#"),
                        other => out.push(other),
                    }
                }
            }
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

fn emit_array(elements: &[Expr], _style: &crate::expr::ArrayStyle) -> String {
    // Crystal doesn't have %i / %w shorthand — render bracket form
    // unconditionally. `style` is preserved for round-trip fidelity
    // in Ruby/Spinel emit but doesn't affect Crystal output.
    if elements.is_empty() {
        // Crystal rejects bare `[]` (no element type to infer).
        // `[] of String` matches the body-typer's `Array(String)`
        // inference for empty-array initializers; nilable-element
        // call sites can still write `[nil]` which inserts a
        // narrower union via the elements' types. Type-mismatched
        // sites surface a Crystal error.
        return "[] of String".to_string();
    }
    let parts: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("[{}]", parts.join(", "))
}

fn emit_hash(entries: &[(Expr, Expr)], kwargs: bool) -> String {
    if kwargs {
        // Kwargs at call site → bare `key: value` shorthand. In Crystal
        // this binds to `**opts` parameters as a NamedTuple. Symbol-keyed
        // entries use the bareword form; hyphenated/special keys quote
        // (`"data-x": v`); non-symbol keys fall back to hashrocket
        // (rare for kwargs but keeps the emitter total).
        let parts: Vec<String> = entries
            .iter()
            .map(|(k, v)| {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                    let name = value.as_str();
                    if is_simple_ident(name) {
                        return format!("{name}: {}", emit_expr(v));
                    }
                    return format!("{:?}: {}", name, emit_expr(v));
                }
                format!("{} => {}", emit_expr(k), emit_expr(v))
            })
            .collect();
        return parts.join(", ");
    }
    // Real Hash literal → `{ :key => value, ... }` hashrocket form.
    // Crystal's `{key: v}` shorthand creates a NamedTuple (compile-time
    // fixed shape, distinct type), so we use the rocket form to force
    // a runtime `Hash(...)`. Preserve the source key type:
    //   - Symbol-typed keys → `:key => value` → Hash(Symbol, V)
    //   - String-typed keys → `"key" => value` → Hash(String, V)
    //   - Generic exprs     → `<expr> => value`
    // Keeping Symbol keys in Crystal matches the framework runtime's
    // expectations (`route[:method]` works against a `Hash(Symbol, V)`,
    // and Crystal Symbols are static so the symbol set is closed —
    // no dynamic-Symbol-creation concern at literal sites).
    if entries.is_empty() {
        // Crystal rejects bare `{}` because it can't infer Hash vs
        // NamedTuple types. Default to `Hash(String, String)` —
        // matches the body-typer's `@h = {}` ivar inference and the
        // typical Rails-shape case.
        return "{} of String => String".to_string();
    }
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                let name = value.as_str();
                // Hyphenated/special-character symbols need the
                // quoted form (`:"data-disable-with"`); plain idents
                // use the bare form (`:foo`). Same convention Ruby
                // and Crystal share.
                if is_simple_ident(name) {
                    return format!(":{name} => {}", emit_expr(v));
                }
                return format!(":{name:?} => {}", emit_expr(v));
            }
            format!("{} => {}", emit_expr(k), emit_expr(v))
        })
        .collect();
    format!("{{ {} }}", parts.join(", "))
}

fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    let mut saw_suffix = false;
    for c in chars {
        if saw_suffix {
            return false;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            continue;
        }
        if matches!(c, '?' | '!' | '=') {
            saw_suffix = true;
            continue;
        }
        return false;
    }
    true
}

pub(super) fn emit_send_base(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Ruby → Crystal method-name translations. Crystal stdlib
    // collections (Array, String, Hash) use `size` not `length`;
    // Ruby has both as aliases. Translate at the call site so
    // emitted code is Crystal-idiomatic.
    // Ruby's `raise Klass, "msg"` is a 2-arg form that Crystal doesn't
    // accept (Crystal's `raise` is single-arg: an Exception or a
    // String). Translate `raise X, "msg"` to `raise X.new("msg")`
    // before any other rewrite. Detected at the bare-method (no recv)
    // call site because the Ruby parser shapes this as a `Send`, not
    // an `ExprNode::Raise`.
    if recv.is_none() && method.as_str() == "raise" && args_s.len() == 2 {
        return format!("raise {}.new({})", args_s[0], args_s[1]);
    }
    // `String.new` (no args) → `""`. Ruby/Spinel `String.new` produces
    // a fresh mutable empty String; Crystal `String.new` exists but
    // takes a Bytes/Slice argument — and Crystal Strings are immutable
    // anyway. The empty string literal `""` is the cross-target
    // accumulator-init the view-body lowerer expects (paired with the
    // `<<` → `io = io + x` rewrite above for appends).
    if method.as_str() == "new" && args.is_empty() {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("String") {
                    return r#""""#.to_string();
                }
            }
        }
    }
    // Ruby `Time` stdlib → Crystal `Time` stdlib bridges.
    //
    // - `Time.now.utc` (Ruby: current time, then convert to UTC) →
    //   `Time.utc` (Crystal: class method returning current UTC time).
    //   Detected as the outer `.utc` Send whose receiver is `Time.now`.
    // - `<expr>.iso8601` (Ruby: ISO-8601 string) → `<expr>.to_rfc3339`
    //   (Crystal's spelling of the same format).
    // Both come from `runtime/ruby/active_record/base.rb`'s
    // `fill_timestamps` chain `Time.now.utc.iso8601`. Spinel/Ruby pass
    // through; only Crystal needs the rewrite since its stdlib uses
    // different method names.
    if method.as_str() == "utc" && args.is_empty() {
        if let Some(r) = recv {
            if let ExprNode::Send { recv: Some(inner), method: inner_m, args: inner_args, .. } = &*r.node {
                if inner_m.as_str() == "now" && inner_args.is_empty() {
                    if let ExprNode::Const { path } = &*inner.node {
                        if path.last().map(|s| s.as_str()) == Some("Time") {
                            return "Time.utc".to_string();
                        }
                    }
                }
            }
        }
    }
    if method.as_str() == "iso8601" && args.is_empty() {
        if let Some(r) = recv {
            return format!("{}.to_rfc3339", emit_expr(r));
        }
    }
    let m = match method.as_str() {
        "length" => "size",
        // Crystal: starts_with? / ends_with? / includes? (note plural).
        // `include?` is method-only — the bare `include` Ruby keyword
        // for module mixin lowers to `LibraryClass::includes`, not
        // a Send, so it's never seen here.
        "start_with?" => "starts_with?",
        "end_with?" => "ends_with?",
        "include?" => "includes?",
        other => other,
    };
    // Ruby's `String#to_sym` dynamically creates Symbols; Crystal
    // doesn't allow runtime Symbol creation. Drop the call entirely
    // (return the receiver) so transpiled code that uses `key.to_sym`
    // for hash-key normalization just stays string-keyed; the
    // primitive runtime treats hash keys as strings throughout.
    if m == "to_sym" && args_s.is_empty() {
        if let Some(r) = recv {
            return emit_expr(r);
        }
    }
    let method_str = m.to_string();
    let method = &method_str;
    let m: &str = method;
    // `recv[idx]` and `recv[idx] = value` rendering. Always emits
    // index-syntax even when receiver is `self` — Ruby's parser shapes
    // `self[k]` as `Send { recv: SelfRef, method: "[]", args: [k] }`,
    // which the SelfRef-collapse below would render as the bare token
    // `[](k)` and Crystal would parse as a malformed empty-array
    // literal. Same reasoning for `self[k] = v` → `Send { method:
    // "[]=", args: [k, v] }`. Drop into index syntax explicitly.
    if (m == "[]" || m == "[]=") && !args_s.is_empty() {
        let recv_s = match recv {
            Some(r) if matches!(&*r.node, ExprNode::SelfRef) => "self".to_string(),
            Some(r) => emit_expr(r),
            None => "self".to_string(),
        };
        if m == "[]=" && args_s.len() == 2 {
            return format!("{recv_s}[{}] = {}", args_s[0], args_s[1]);
        }
        return format!("{recv_s}[{}]", args_s.join(", "));
    }

    if matches!(recv, Some(r) if matches!(&*r.node, ExprNode::SelfRef))
        && !is_setter_method(m)
        && !is_binary_operator(m)
        && !super::shared::is_crystal_reserved(m)
    {
        if args_s.is_empty() {
            return method.to_string();
        }
        if parenthesized {
            return format!("{method}({})", args_s.join(", "));
        }
        return format!("{method} {}", args_s.join(", "));
    }
    match (recv, m) {
        (Some(r), "[]") => format!("{}[{}]", emit_expr(r), args_s.join(", ")),
        (Some(r), op) if is_binary_operator(op) && args_s.len() == 1 => {
            format!("{} {op} {}", emit_expr(r), args_s[0])
        }
        (Some(r), name) if is_setter_method(name) && args_s.len() == 1 => {
            let attr = &name[..name.len() - 1];
            format!("{}.{attr} = {}", emit_expr(r), args_s[0])
        }
        (None, _) => {
            if args_s.is_empty() {
                method.to_string()
            } else if parenthesized {
                format!("{method}({})", args_s.join(", "))
            } else {
                format!("{method} {}", args_s.join(", "))
            }
        }
        (Some(r), _) => {
            let recv_s = emit_expr(r);
            // Wrap low-precedence receivers (e.g. `a || b`, `a && b`,
            // assignments, ternaries) in parens so the dot binds to
            // the whole expression — `(a || b).to_s` not the natural
            // parse `a || b.to_s`. The IR carries the source-grouping
            // intent (the `(... ||  ...).to_s` shape lowers to
            // Send{recv: BoolOp, ...}); preserve that grouping in
            // the emit.
            let recv_s = if needs_recv_parens(r) {
                format!("({recv_s})")
            } else {
                recv_s
            };
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else if parenthesized {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            } else {
                format!("{recv_s}.{method} {}", args_s.join(", "))
            }
        }
    }
}

/// True when an expression's natural emit would have lower precedence
/// than the surrounding `recv.method` dot — wrap it in parens at the
/// recv position so the dot binds to the whole expression. Conservative:
/// only flags the cases we've actually hit (BoolOp + If/ternary).
fn needs_recv_parens(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::BoolOp { .. } | ExprNode::If { .. } | ExprNode::Assign { .. }
    )
}

fn is_binary_operator(m: &str) -> bool {
    matches!(
        m,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "<=>"
            | "==="
            | "=~"
            | "!~"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "<<"
            | ">>"
            | "&"
            | "|"
            | "^"
    )
}

/// Translate Ruby's `raise Klass, "msg"` (parsed as Send `raise` with
/// two args) to Crystal's `raise Klass.new("msg")`. Single-arg raises
/// (`raise "msg"` or `raise exc`) pass through unchanged.
fn emit_raise(value: &Expr) -> String {
    if let ExprNode::Send { recv: None, method, args, .. } = &*value.node {
        if method.as_str() == "raise" && args.len() == 2 {
            // Inner Send shape: `raise(Klass, "msg")`. Convert.
            let klass_s = emit_expr(&args[0]);
            let msg_s = emit_expr(&args[1]);
            return format!("raise {klass_s}.new({msg_s})");
        }
    }
    // Heuristic fallback: an Apply or Send-with-recv that produces
    // a (Klass, msg) pair would still need handling; for now just
    // emit the single-value form.
    format!("raise {}", emit_expr(value))
}

fn is_setter_method(m: &str) -> bool {
    if !m.ends_with('=') || m.len() < 2 {
        return false;
    }
    if matches!(m, "==" | "!=" | "<=" | ">=" | "<=>" | "===" | "=~") {
        return false;
    }
    if m == "[]=" {
        return false;
    }
    true
}

pub(super) fn emit_do_block(base: &str, block: &Expr) -> String {
    use crate::expr::BlockStyle;
    let ExprNode::Lambda { params, body, block_style, .. } = &*block.node else {
        return format!("{base} {{ {} }}", emit_expr(block));
    };
    let body_str = emit_expr(body);
    let params_str = if params.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        format!(" |{}|", ps.join(", "))
    };
    match block_style {
        BlockStyle::Brace => {
            format!("{base} {{{params_str} {body_str} }}")
        }
        BlockStyle::Do => emit_do_form(base, &params_str, &body_str),
    }
}

fn emit_do_form(base: &str, params_str: &str, body_str: &str) -> String {
    let params_clause = if params_str.is_empty() {
        "do".to_string()
    } else {
        format!("do{params_str}")
    };
    if body_str.contains('\n') {
        format!(
            "{base} {}\n{}\nend",
            params_clause,
            indent_lines(body_str, 1),
        )
    } else {
        format!("{base} {} {} end", params_clause, body_str)
    }
}

pub(super) fn emit_literal(l: &Literal) -> String {
    match l {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!(":{value}"),
        Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
    }
}

fn emit_lvalue(lv: &LValue) -> String {
    match lv {
        LValue::Var { name, .. } => escape_ident(name.as_str()),
        LValue::Ivar { name } => format!("@{name}"),
        LValue::Attr { recv, name } => format!("{}.{name}", emit_expr(recv)),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
    }
}

fn emit_arm(arm: &Arm) -> String {
    let mut s = format!("when {}", emit_pattern(&arm.pattern));
    if let Some(g) = &arm.guard {
        s.push_str(&format!(" if {}", emit_expr(g)));
    }
    s.push('\n');
    s.push_str(&indent_lines(&emit_expr(&arm.body), 1));
    s.push('\n');
    s
}

fn emit_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Bind { name } => name.to_string(),
        Pattern::Lit { value } => emit_literal(value),
        Pattern::Array { elems, rest } => {
            let mut parts: Vec<String> = elems.iter().map(emit_pattern).collect();
            if let Some(r) = rest {
                parts.push(format!("*{r}"));
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record { fields, rest } => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", emit_pattern(v)))
                .collect();
            if *rest {
                parts.push("**".into());
            }
            format!("{{ {} }}", parts.join(", "))
        }
    }
}
