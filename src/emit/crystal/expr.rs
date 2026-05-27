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

use crate::expr::{Arm, Expr, ExprNode, InterpPart, IrHint, LValue, Literal, Pattern};
use crate::ident::Symbol;
use crate::ty::Ty;

use super::shared::{escape_ident, indent_lines};

thread_local! {
    /// True while rendering the body of a synth `[]` reader. The
    /// auto-`.not_nil!` Ivar bridge below would crash on unset
    /// columns of fresh-from-`new` records; suppress it for the
    /// duration of the index-reader body. `emit_method` toggles this
    /// flag on entry to a method named `[]` and clears on exit.
    static SUPPRESS_IVAR_NOT_NIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn with_suppressed_ivar_not_nil<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = SUPPRESS_IVAR_NOT_NIL.with(|c| c.replace(true));
    let r = f();
    SUPPRESS_IVAR_NOT_NIL.with(|c| c.set(prev));
    r
}

pub fn emit_expr(e: &Expr) -> String {
    // IrHint::StringBuilder* — lowerer-tagged accumulator triple
    // (`io = String.new; io << "..."; io`). Crystal's `String + String`
    // is O(n²) per append (immutable Strings reallocate); swap to
    // `String::Builder` which writes in place via `IO#<<`. Single
    // hook at the entry of emit_expr so all three sites — Assign,
    // Send, terminal Var — get rewritten in one place.
    if let Some(s) = try_string_builder(e) {
        return s;
    }
    // Empty Hash/Array literal with a concrete type annotation —
    // render with the `of K => V` / `of E` clause so Crystal infers
    // the container type from the annotation rather than the default
    // `{} of String => String` / `[] of String`. Driven by the
    // body-typer's expected-type propagation in
    // `propagate_expected_to_empty_container` (Assign branch).
    if let Some(ty) = e.ty.as_ref() {
        match (&*e.node, ty) {
            (ExprNode::Hash { entries, kwargs: false }, Ty::Hash { key, value })
                if entries.is_empty() =>
            {
                return format!(
                    "{{}} of {} => {}",
                    super::ty::crystal_ty(key),
                    super::ty::crystal_ty(value),
                );
            }
            (ExprNode::Array { elements, .. }, Ty::Array { elem })
                if elements.is_empty() =>
            {
                return format!("[] of {}", super::ty::crystal_ty(elem));
            }
            _ => {}
        }
    }
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
            if is_non_nilable_concrete(ty)
                && !SUPPRESS_IVAR_NOT_NIL.with(|c| c.get())
            {
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

/// Strip a trailing `Nil` variant from a binary `Union { T, Nil }`,
/// returning the `T` arm. Larger unions and non-union types pass
/// through unchanged. Used to see through ivar reads that the body-
/// typer surfaced as nilable (`@slots` first read observes nil).
fn unwrap_nilable_union(ty: &crate::ty::Ty) -> &crate::ty::Ty {
    use crate::ty::Ty;
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            let nil_idx = variants.iter().position(|v| matches!(v, Ty::Nil));
            if let Some(idx) = nil_idx {
                return &variants[1 - idx];
            }
        }
    }
    ty
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
/// True when `recv` is a framework hash-like class (`Parameters`,
/// `HashWithIndifferentAccess`, `ActiveSupport::HashWithIndifferentAccess`)
/// — these classes expose `key?` directly and shouldn't get the
/// `key?` → `has_key?` Crystal-Hash rewrite.
fn is_framework_hash_recv(recv: Option<&crate::expr::Expr>) -> bool {
    let Some(r) = recv else { return false; };
    let Some(ty) = r.ty.as_ref() else { return false; };
    if let crate::ty::Ty::Class { id, .. } = ty {
        let name = id.0.as_str();
        let last = name.rsplit("::").next().unwrap_or(name);
        return matches!(
            last,
            "Parameters" | "Flash" | "Session"
        );
    }
    false
}

fn rewrite_stdlib_const(name: &str) -> Option<&'static str> {
    match name {
        "Integer" => Some("Int"),
        // Crystal's parent for Int/Float is `Number`, not Ruby's
        // `Numeric`. The framework runtime uses `is_a?(Numeric)` for
        // the numericality validator's int+float check; rewrite the
        // bare const so the predicate compiles.
        "Numeric" => Some("Number"),
        // Ruby's StandardError / RuntimeError have no Crystal analog;
        // collapse to `Exception` (the matching base class). Mirrors
        // the parent-class swap in `crystal_parent_name`. Without
        // this, `assert_operator(X, :<, StandardError)` and similar
        // bare-Const refs leave an undefined name in the output.
        "StandardError" | "RuntimeError" => Some("Exception"),
        // The DB-bridge runtime module lives at `Roundhouse::Db`
        // (see `runtime/crystal/db.cr`). Model code emits at top
        // level (`class Article < ApplicationRecord`), so a bare
        // `Db.prepare(...)` reference resolves in Crystal's
        // namespace lookup to nothing (and the diagnostic
        // confusingly suggests `DB`, the stdlib database module).
        // Re-qualify here so `Db.prepare`/`Db.escape_int`/`Db.step?`
        // all reach the runtime helpers.
        "Db" => Some("Roundhouse::Db"),
        _ => None,
    }
}

/// String-accumulator hint consumer. The lowerer (view_to_library /
/// jbuilder_to_library) tags the three sites of its `io = String.new;
/// io << "..."; io` pattern with `IrHint::StringBuilder*`; this
/// helper rewrites them to Crystal's `String::Builder` idiom so the
/// inner concat stays O(n) instead of O(n²).
///
/// - `Init` rewrites the whole `Assign { Var, String.new }` to
///   `<var> = String::Builder.new`, bypassing the value-side `<<`
///   shovel rewrite (which the lowerer's `<<` Send carries instead).
/// - `Append` keeps the source-faithful `<var> << <arg>` (Crystal's
///   `IO#<<` works on Builder directly), bypassing the otherwise-
///   applied `<var> = <var> + <arg>` rewrite for Str-typed recv.
/// - `Result` rewrites the terminal Var to `<var>.to_s`, so the
///   function returns the finalized String.
///
/// Non-hinted sites (user-authored Ruby outside lowerer synthesis)
/// still take the legacy `String + String` path — the hint is the
/// signal that the lowerer guarantees Builder semantics are safe.
fn try_string_builder(e: &Expr) -> Option<String> {
    match e.hint? {
        IrHint::StringBuilderInit => {
            if let ExprNode::Assign {
                target: LValue::Var { name, .. }, ..
            } = &*e.node
            {
                return Some(format!(
                    "{} = String::Builder.new",
                    escape_ident(name.as_str())
                ));
            }
            None
        }
        IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        let var = escape_ident(name.as_str());
                        let val = emit_expr(&args[0]);
                        return Some(format!("{var} << {val}"));
                    }
                }
            }
            None
        }
        IrHint::StringBuilderResult => {
            if let ExprNode::Var { name, .. } = &*e.node {
                return Some(format!("{}.to_s", escape_ident(name.as_str())));
            }
            None
        }
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
            // Multi-segment paths whose first element is a known
            // framework module qualify to absolute (`::ActiveRecord::
            // Base`); single-segment app refs (`Article`) stay bare.
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
            if path.len() == 1 {
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
            // `@ivar.nil?` — suppress the auto-`.not_nil!` that the
            // Ivar narrowing rule (above) inserts on non-nilable
            // concrete-typed ivar reads. Schema-derived column
            // accessors type their ivar as `T` (the setter's value
            // type after the `.as?(T).not_nil!` chain in
            // `synth_attr_writer`), but the property declaration is
            // `T?` and the runtime ivar legitimately holds nil for
            // freshly-allocated records. `@x.not_nil!.nil?` would
            // raise instead of returning true — the `.nil?` check is
            // the canonical pre-validation guard in `validate`
            // bodies. Same suppression for `.is_a?(NilClass)` (an
            // equivalent nil-introspection shape).
            if args.is_empty() && method.as_str() == "nil?" {
                if let Some(r) = recv {
                    if let ExprNode::Ivar { name } = &*r.node {
                        return format!("@{name}.nil?");
                    }
                }
            }
            // `<Class>.new({k: v, ...})` / `<Class>.create({k: v, ...})`
            // — user-test idiom (`Comment.new({article_id: x, ...})`)
            // and ActiveRecord factory (`Comment.create({...})`). The
            // synthesized `def initialize(attrs)` is dropped on the
            // Crystal side (its `Hash[Sym, Untyped]` lookups don't
            // reconcile with typed setters); rewrite the call site
            // into a `begin … end` expression that builds the
            // instance with per-field setters, mirroring the shape
            // the fixture lowerer uses. Each setter has its declared
            // type, so the hash-value union never surfaces.
            //
            // Only fires for user-defined class names (capitalized
            // first char, not a stdlib container) with a single
            // non-empty Hash literal argument whose keys are all
            // simple-Symbol literals. Hand-written calls that
            // already pass a typed Hash variable, or non-Hash args,
            // fall through to the standard emit.
            if (method.as_str() == "new" || method.as_str() == "create")
                && args.len() == 1
            {
                if let Some(r) = recv {
                    if let Some(rewrite) =
                        try_emit_new_or_create_per_field(r, method.as_str(), &args[0])
                    {
                        return rewrite;
                    }
                }
            }
            // `v.is_a?(TrueClass)` / `is_a?(FalseClass)` — Ruby
            // distinguishes the singleton classes of `true` / `false`;
            // Crystal has neither. Rewrite to `== true` / `== false`
            // (works under any receiver type — value comparison).
            // `is_a?(NilClass)` similarly rewrites to `== nil`. The
            // jbuilder runtime's polymorphic value classifier
            // (`runtime/ruby/json_builder.rb`) is the canonical
            // producer.
            //
            // `is_a?(Hash)` widens to `(... is_a?(Hash)) ||
            // (... is_a?(NamedTuple))` — the framework's `data: {...}`
            // attribute shape arrives as a Crystal NamedTuple (the
            // `{key: val}` shorthand emits NamedTuple syntax), which
            // `is_a?(Hash)` alone would reject. The unwrap path in
            // `ViewHelpers.render_attrs` expects both shapes; the
            // Ruby source can't say "either Hash or NamedTuple"
            // because NamedTuple is Crystal-specific.
            if method.as_str() == "is_a?" && args.len() == 1 {
                if let ExprNode::Const { path } = &*args[0].node {
                    if let Some(last) = path.last() {
                        let lit = match last.as_str() {
                            "TrueClass" => Some("true"),
                            "FalseClass" => Some("false"),
                            "NilClass" => Some("nil"),
                            _ => None,
                        };
                        if let (Some(r), Some(lit)) = (recv, lit) {
                            return format!("{} == {lit}", emit_expr(r));
                        }
                        if last.as_str() == "Hash" {
                            if let Some(r) = recv {
                                let r_s = emit_expr(r);
                                return format!(
                                    "({r_s}.is_a?(Hash) || {r_s}.is_a?(NamedTuple))"
                                );
                            }
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
        // Crystal supports the same compound-assignment surface as
        // Ruby (`||=`, `&&=`, `+=`, …) with matching short-circuit
        // semantics.
        ExprNode::OpAssign { target, op, value } => {
            format!("{} {} {}", emit_lvalue(target), op.as_ruby(), emit_expr(value))
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
        ExprNode::Break { value } => match value {
            None => "break".to_string(),
            Some(v) => format!("break {}", emit_expr(v)),
        },
        ExprNode::Splat { value } => format!("*{}", emit_expr(value)),
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
                // Crystal rescue syntax differs from Ruby:
                //   - `rescue Class` (no binding)
                //   - `rescue binding : Class` (binding REQUIRES a class
                //     type; no bare `rescue => binding` form)
                //   - Multiple classes: `rescue binding : C1 | C2`
                s.push_str("rescue");
                let cs: Vec<String> = rc.classes.iter().map(emit_expr).collect();
                if let Some(name) = &rc.binding {
                    let ty = if cs.is_empty() {
                        "Exception".to_string()
                    } else {
                        cs.join(" | ")
                    };
                    s.push_str(&format!(" {name} : {ty}"));
                } else if !cs.is_empty() {
                    s.push(' ');
                    s.push_str(&cs.join(", "));
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
            // Crystal `.as?(T).not_nil!` — runtime-checked downcast
            // that survives Crystal's per-call-site monomorphization.
            // Plain `.as(T)` rejects at compile time when the value's
            // monomorphized type can't be `T` (e.g., a model's `[]=`
            // method called from `fill_timestamps` with `value: String`
            // can't cast to `Int64` for the `:id` arm — even if that
            // arm is unreachable for that call). `.as?(T)` returns
            // `T?` regardless of static type and is decided at
            // runtime; `.not_nil!` then re-asserts non-nil on the
            // expected branch. Wraps the value in parens so chained
            // casts and operator-precedence edges parse correctly.
            format!(
                "({}).as?({}).not_nil!",
                emit_expr(value),
                super::ty::crystal_ty(target_ty)
            )
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
    // Hash#[] safety bridge for `||`-default idiom: Ruby's
    // `hash[k] || default` returns the default for missing keys
    // because Hash#[] returns nil there. Crystal's strict Hash#[]
    // raises KeyError instead — the LHS never gets a chance to be
    // nil. Detect the pattern at emit (LHS is `Hash#[](k)` Send) and
    // rewrite to `hash[k]?` so the missing-key path returns nil and
    // `||` falls through to the default. Only the read-form `[]` is
    // rewritten — `[]=` and other methods keep their shape.
    if matches!(op, BoolOpKind::Or) {
        if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*left.node {
            if method.as_str() == "[]" && !args.is_empty() {
                let recv_ty = recv.ty.as_ref().map(unwrap_nilable_union);
                if matches!(recv_ty, Some(crate::ty::Ty::Hash { .. })) {
                    let recv_s = emit_expr(recv);
                    let arg_strs: Vec<String> = args.iter().map(emit_expr).collect();
                    return format!("{}[{}]? {} {}", recv_s, arg_strs.join(", "), op_s, emit_expr(right));
                }
            }
        }
    }
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

/// True when the call-site `recv.method(args...)` has a known Hash
/// (not NamedTuple) param shape. Used to override the Hash literal
/// emit's all-symbol-keys → NamedTuple default for arg positions
/// where Crystal expects a runtime Hash. Targets specific framework
/// constructors that take a `Hash[untyped, untyped]` per RBS.
/// Rewrite `<Class>.new(<hash literal>)` / `<Class>.create(<hash
/// literal>)` to a `begin … end` block that instantiates and
/// assigns each field via its typed setter. The Crystal-side
/// synth_initialize is intentionally skipped (its
/// `Hash[Sym, Untyped]` lookups don't reconcile with typed
/// setters); this rewrite re-routes the call sites that needed
/// the hash form. Returns `None` for shapes that don't match —
/// caller falls back to the standard emit.
///
/// Only fires for:
///   - User-defined class names (capitalized first char, not a
///     stdlib container).
///   - A single argument that's a non-empty Hash literal with all
///     simple-Symbol keys.
///
/// `.new` produces: `(__inst = Cls.new(hash); __inst.k1 = v1; …; __inst)`
/// `.create` adds an explicit `.save` after the assigns and still
/// returns the instance.
///
/// The hash is passed through to `.new` (in addition to the per-
/// field assigns) so classes with a user-defined `initialize(attrs)`
/// — framework_tests' `HashItem` shape — still see the hash and
/// set fields from it. The trailing per-field setters are
/// redundant for those classes but harmless (same value re-assigned
/// via the typed setter); for AR-inherited models whose
/// `initialize` ignores attrs, the setters are what actually
/// populate the record. Both shapes converge.
fn try_emit_new_or_create_per_field(
    recv: &Expr,
    method: &str,
    arg: &Expr,
) -> Option<String> {
    let ExprNode::Const { path } = &*recv.node else {
        return None;
    };
    let class_name = path
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("::");
    let last = path.last()?.as_str();
    let is_stdlib_container = matches!(
        last,
        "Hash"
            | "Array"
            | "Tuple"
            | "NamedTuple"
            | "Set"
            | "Range"
            | "Slice"
            | "Bytes"
            | "String"
            | "Object"
            | "Class"
            | "Exception"
    );
    let starts_upper = last
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase());
    if !starts_upper || is_stdlib_container {
        return None;
    }
    // The Hash literal arg must be present, non-empty, and have
    // simple-Symbol keys throughout — that's the user-test /
    // fixture / seed shape this rewrite targets.
    let ExprNode::Hash { entries, kwargs: false } = &*arg.node else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    let mut field_assigns: Vec<(String, String)> = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*k.node else {
            return None;
        };
        let name = sym.as_str();
        if !is_simple_method_name(name) {
            return None;
        }
        field_assigns.push((name.to_string(), emit_expr(v)));
    }
    // Emit the original hash literal (in hashrocket form so it
    // satisfies AR-inherited `Hash[Symbol, _]` signatures rather
    // than Crystal's NamedTuple shorthand) so user-defined
    // `initialize(attrs)` still sees the data. Use the existing
    // forced-hash emitter via `emit_expr_with_form_hint` so nested
    // hash values flip too.
    let hash_arg_s = emit_expr_with_form_hint(arg, true);
    let mut body = String::new();
    body.push_str(&format!("__inst = {}.new({})\n", class_name, hash_arg_s));
    for (field, value_s) in &field_assigns {
        body.push_str(&format!("__inst.{} = {}\n", field, value_s));
    }
    if method == "create" {
        body.push_str("__inst.save\n");
    }
    body.push_str("__inst");
    Some(format!("begin\n{}\nend", indent_lines(&body, 1)))
}

fn force_hash_form_for_arg(recv: Option<&Expr>, method: &Symbol) -> bool {
    if method.as_str() != "new" {
        return false;
    }
    let Some(r) = recv else { return false; };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    // Per-class constructor shapes that take a `Hash[untyped,
    // untyped]` per RBS. ActiveRecord::Base.initialize is the
    // primary case — model `<Plural>Fixtures._fixtures_load!` builds
    // `Article.new({id: 1, title: …})` and the auto-NamedTuple emit
    // would mismatch the Hash-typed `_attrs` parameter. We can't
    // enumerate every app-defined model here (Crystal emit doesn't
    // carry the App context), so the catch-all condition is "the
    // recv looks like a user-named class (capitalized Const, not in
    // the stdlib/framework drop list) and the only arg is a Hash
    // literal with simple-Symbol keys" — checked at the arg-emit
    // site via `force_hash_form_for_arg_with_hash_arg`. The named
    // entries below stay for the framework-only types that emit
    // before the model-emit pipeline runs.
    let last = path.last().map(|s| s.as_str());
    if matches!(
        last,
        Some("Parameters") | Some("Flash") | Some("Session")
    ) {
        return true;
    }
    // User-defined class names — capitalized first char, no `::`
    // beyond a single segment, NOT a stdlib container. The fixture
    // loader and any other `Model.new(attrs_hash)` call site fits
    // this shape. NamedTuple-shaped Crystal stdlib targets (`Tuple`,
    // `Set`, `Range`, …) are excluded so a hand-written
    // `Tuple.new({a: 1})` doesn't get flipped.
    if let Some(name) = last {
        let is_stdlib_container = matches!(
            name,
            "Hash"
                | "Array"
                | "Tuple"
                | "NamedTuple"
                | "Set"
                | "Range"
                | "Slice"
                | "Bytes"
                | "String"
        );
        let starts_upper = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if starts_upper && !is_stdlib_container {
            return true;
        }
    }
    false
}

/// `emit_expr` variant that passes a "force Hash form" hint into the
/// emitted Hash literal. The hint only fires for the immediate Hash
/// literal at the arg position; deeper nested literals keep their
/// own emit decision.
fn emit_expr_with_form_hint(e: &Expr, force_hash: bool) -> String {
    if !force_hash {
        return emit_expr(e);
    }
    if let ExprNode::Hash { entries, kwargs } = &*e.node {
        if !kwargs {
            return emit_hash_forced(entries);
        }
    }
    emit_expr(e)
}

/// Emit a Hash literal in hashrocket form, even when all keys are
/// simple-ident Symbols (which would otherwise emit as NamedTuple
/// shorthand). Used at call sites where Crystal's NamedTuple type
/// would mismatch the receiver's expected `Hash[K, V]` parameter.
/// Recurses into nested Hash literals so the entire literal tree
/// stays in Hash form (`Parameters.new({:a => {:b => "c"}})`
/// keeps `{:b => "c"}` as a Hash too — needed because the
/// outer constructor walks values and re-wraps nested Hashes
/// into Parameters at runtime).
fn emit_hash_forced(entries: &[(Expr, Expr)]) -> String {
    if entries.is_empty() {
        return "{} of String => String".to_string();
    }
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            let v_s = emit_expr_with_form_hint_forced(v);
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                let name = value.as_str();
                if is_simple_ident(name) {
                    return format!(":{name} => {v_s}");
                }
                return format!(":{:?} => {v_s}", name);
            }
            format!("{} => {v_s}", emit_expr(k))
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Emit `e`, recursively forcing Hash-form inside any nested
/// Hash literal. Mirrors the recursive-normalize semantics of
/// `Parameters.new` / `HashWithIndifferentAccess.new` — the outer
/// wrapper walks values and constructs nested Parameters/HWIA
/// from each Hash; passing NamedTuple at any depth would break
/// the recursion.
fn emit_expr_with_form_hint_forced(e: &Expr) -> String {
    if let ExprNode::Hash { entries, kwargs } = &*e.node {
        if !kwargs {
            return emit_hash_forced(entries);
        }
    }
    emit_expr(e)
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
    // Hash literal emit. Two forms:
    //   - `{key: v, key2: v}` (NamedTuple shorthand) — used when
    //     every key is a Symbol literal with a simple-ident name.
    //     Crystal infers a NamedTuple type from the literal,
    //     preserving per-key types. Required for typed-record
    //     receivers (Router.match's typed return shape) where a
    //     `Hash(Symbol, V)` value-union would collapse the per-key
    //     types into the union.
    //   - `{:key => v, "k" => v}` (hashrocket) — used otherwise.
    //     Forces a runtime `Hash(...)` with key/value type unions.
    // Empty hashes have no key-type evidence; default to
    // `Hash(String, String)` so subsequent `[]=` writes typecheck.
    if entries.is_empty() {
        return "{} of String => String".to_string();
    }
    let all_symbol_simple_keys = entries.iter().all(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if is_simple_ident(value.as_str()))
    });
    if all_symbol_simple_keys {
        let parts: Vec<String> = entries
            .iter()
            .map(|(k, v)| {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
                    unreachable!()
                };
                format!("{}: {}", value.as_str(), emit_expr(v))
            })
            .collect();
        return format!("{{{}}}", parts.join(", "));
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
    let args_s: Vec<String> = args
        .iter()
        .map(|a| emit_expr_with_form_hint(a, force_hash_form_for_arg(recv, method)))
        .collect();
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
    // Unary `!` Send (`Send { recv: cond, method: "!", args: [] }`) →
    // prefix form `!(cond)`. Wrap the operand in parens — `!` binds
    // tighter than binary operators (`<`, `==`, etc.), so
    // `!recv.op(arg)` would parse as `(!recv) < arg` when the inner
    // Send emits as infix (`A < B`). Explicit parens preserve the
    // intended `!(recv.op(arg))`. Mirrors the same arm in Ruby's
    // emit (src/emit/ruby/expr.rs).
    if method.as_str() == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!({})", emit_expr(r));
        }
    }
    // Cross-target nil-safe Hash read: Ruby `h.fetch(key, nil)` returns
    // the value or nil for missing key. Crystal's `Hash#fetch(K, V)`
    // works for Hash recv — but the same source file may be reached
    // with NamedTuple recv (call site passes `{a: 1}` literal), and
    // `NamedTuple#fetch(K, V)` doesn't exist (only the block form
    // does, and it widens the result type to the union of the
    // NamedTuple's value types — useless for option-hash patterns).
    // Translate to `recv[K]?` which works on both Hash and
    // NamedTuple and returns a clean nilable. Only fires when the
    // default arg is the literal `nil` so the source intent is
    // unambiguous; other defaults flow through the standard
    // `recv.fetch(K, default)` emit.
    if method.as_str() == "fetch" && args.len() == 2 {
        if let ExprNode::Lit { value: Literal::Nil } = &*args[1].node {
            if let Some(r) = recv {
                let recv_s = emit_expr(r);
                return format!("{recv_s}[{}]?", args_s[0]);
            }
        }
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
    // Ruby `JSON.generate(obj)` → Crystal `obj.to_json`. Both produce
    // a JSON String from a serializable value, but Crystal exposes
    // the API as an instance method rather than a module function.
    // Pattern-match the source-shape `JSON.generate(x)` Send (recv =
    // Const JSON, single arg) and rewrite to the receiver-flipped
    // form. `JSON.parse` keeps the same shape in both.
    if method.as_str() == "generate" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("JSON") {
                    return format!("{}.to_json", emit_expr(&args[0]));
                }
            }
        }
    }
    // Ruby `Base64.{strict_encode64,encode64,...}` → Crystal's
    // `Base64.{strict_encode,encode,...}` (drop the `64` suffix —
    // Crystal's Base64 module dropped it for ergonomics).
    let base64_recv = matches!(
        recv,
        Some(r) if matches!(&*r.node, ExprNode::Const { path } if path.last().map(|s| s.as_str()) == Some("Base64"))
    );
    let m = match method.as_str() {
        "length" => "size",
        // Ruby `Array#count` (no args) → element count. Crystal's
        // `Enumerable#count` requires either an item to match or a
        // block — `arr.count` (no args) doesn't compile. Rewrite to
        // `size`. The argful form (`count(item)`) passes through
        // unchanged below.
        //
        // Skip the rewrite for Const receivers: `Item.count` is a
        // class method (e.g. `ActiveRecord::Base.count` calls the
        // adapter), not an Enumerable. `Item.size` would dispatch
        // against the metaclass and not resolve.
        "count" if args_s.is_empty()
            && !matches!(recv, Some(r) if matches!(&*r.node, ExprNode::Const { .. }))
            => "size",
        // Ruby `Hash#key?(k)` exists; Crystal's stdlib Hash uses
        // `has_key?(k)` (with `key?` not exposed for direct dispatch).
        // Rewrite at the call site so transpiled Ruby Hash idioms
        // compile — but framework classes (Parameters, HWIA) define
        // `key?` explicitly as part of their API, so skip the
        // rewrite when the receiver is one of those.
        "key?" if !is_framework_hash_recv(recv) => "has_key?",
        // Crystal: starts_with? / ends_with? / includes? (note plural).
        // `include?` is method-only — the bare `include` Ruby keyword
        // for module mixin lowers to `LibraryClass::includes`, not
        // a Send, so it's never seen here.
        "start_with?" => "starts_with?",
        "end_with?" => "ends_with?",
        "include?" => "includes?",
        // Ruby `Regexp#match?(str)` → Crystal `Regex#matches?(str)`
        // (note plural). Both predicate-only forms — return Bool.
        // No receiver-type narrowing here: the only Ruby class with
        // a `match?` method that takes a single String is Regexp;
        // String#match?(re) also exists but we don't emit that
        // pattern. Safe to translate unconditionally.
        "match?" if args_s.len() == 1 => "matches?",
        "strict_encode64" if base64_recv => "strict_encode",
        "strict_decode64" if base64_recv => "strict_decode",
        "encode64" if base64_recv => "encode",
        "decode64" if base64_recv => "decode",
        "urlsafe_encode64" if base64_recv => "urlsafe_encode",
        "urlsafe_decode64" if base64_recv => "urlsafe_decode",
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
    //
    // Symbol-key → String-key auto-conversion: when the receiver is a
    // bound name (Var/Ivar/Send result) typed as `Hash` and the index
    // is a Symbol literal, emit the String form. Crystal's
    // SqliteAdapter returns `Hash(String, DB::Any)` rows; the IR
    // types row params as `Hash[Sym, Untyped]` (matching Ruby's
    // symbol-keyed convention), so the Symbol literal at the [] site
    // would dispatch String-keyed Crystal Hash#[] with a Symbol arg
    // and KeyError. Ruby sources that read a row via `row[:id]`
    // transparently route through this conversion. Hash literal
    // receivers (`{:a => 1}[:a]`) skip — those Hashes are actually
    // Symbol-keyed at runtime.
    if (m == "[]" || m == "[]=") && !args_s.is_empty() {
        let recv_s = match recv {
            Some(r) if matches!(&*r.node, ExprNode::SelfRef) => "self".to_string(),
            Some(r) => emit_expr(r),
            None => "self".to_string(),
        };
        // Hash receiver behavior bridges Ruby/Crystal divergence:
        //   - Symbol-literal index on a `Hash[Str, V]` recv: convert
        //     `:k` → `"k"`. Adapter rows are `Hash(String, DB::Any)`
        //     at runtime; `row[:id]` would dispatch
        //     `Hash(String,V)#[](Symbol)` and KeyError.
        //   - Read on any Hash recv: emit `[k]?` instead of `[k]`.
        //     Ruby Hash#[] returns nil for missing keys; Crystal
        //     Hash#[] raises. The transpiled framework runtime
        //     (button_to's `opts[:method]`, ViewHelpers helpers)
        //     relies on the nil-default. Hash literals with the same
        //     IR type get the same treatment, which is right — a
        //     missing key on `{ :a => 1 }[:b]` should be nil-shaped,
        //     not crash, to match Ruby. `[]?` returns `V?`, which
        //     propagates as a nilable type to the call site (the
        //     receiver's typed call sites already wrap `.not_nil!`
        //     where required).
        // Treat `T` and `T | Nil` uniformly: the body-typer often
        // surfaces ivars as `Union { Hash, Nil }` (e.g., `@slots` in
        // ViewHelpers reads can observe nil before the first
        // assignment). The recv emit may already have wrapped with
        // `.not_nil!`, but the IR's `e.ty` is still nilable. Strip a
        // trailing `Nil` from a binary union so the key-conversion
        // detection below sees through the nilable.
        let recv_ty = recv.and_then(|r| r.ty.as_ref()).map(unwrap_nilable_union);
        // Ivar Hash key types from the body-typer are unreliable —
        // empty-hash initializers (`@slots = {}`) collapse to the
        // `Hash<Str, Str>` default, even when subsequent writes use
        // Symbol keys. The Crystal class-var declaration emit derives
        // the type independently from `@var[k] = v` index-assign sites
        // and gets it right (Hash<Symbol, V>); converting Symbol → Str
        // here would mismatch that declaration. Adapter rows (the
        // original motivation for the conversion) are method
        // parameters / locals (Var), not Ivars, so this restriction
        // doesn't lose coverage there.
        let recv_is_ivar = matches!(recv.map(|r| &*r.node), Some(ExprNode::Ivar { .. }));
        let recv_str_keyed = !recv_is_ivar && matches!(
            recv_ty,
            Some(crate::ty::Ty::Hash { key, .. }) if matches!(**key, crate::ty::Ty::Str)
        );
        let mut converted: Vec<String> = args_s.clone();
        if recv_str_keyed {
            for (idx, a) in args.iter().enumerate() {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node {
                    converted[idx] = format!("\"{}\"", value.as_str());
                }
            }
        }
        if m == "[]=" && converted.len() == 2 {
            return format!("{recv_s}[{}] = {}", converted[0], converted[1]);
        }
        return format!("{recv_s}[{}]", converted.join(", "));
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
            // Hash-flavored receiver methods: when the recv is a
            // bare `{a: 1, b: 2}` literal and the method is one
            // Crystal's NamedTuple doesn't bridge to Hash for
            // (`merge` with a Hash arg is the canonical case —
            // NamedTuple#merge requires another NamedTuple, not a
            // Hash), force the recv literal into Hash form. Without
            // this, `{href: href}.merge(opts)` collapses to
            // `NamedTuple#merge(Hash)` which Crystal rejects. Ruby
            // semantics: the literal is a Hash; merging with another
            // Hash is fine. NamedTuple is a Crystal-only type.
            let force_recv_hash = matches!(method.as_str(), "merge" | "merge!" | "update")
                && matches!(&*r.node, ExprNode::Hash { kwargs: false, .. });
            let recv_s = if force_recv_hash {
                emit_expr_with_form_hint(r, true)
            } else {
                emit_expr(r)
            };
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
            // Auto-`.not_nil!` for chained dispatch through a nilable
            // class-or-nil receiver: `Article.last.title` arrives as
            // `Send{recv: Send{...last}, method: title}` where the
            // inner send's `Ty::Union<Article, Nil>` would compile-
            // fail under Crystal's strict null-check. The Rails idiom
            // assumes presence; surfacing `.not_nil!` matches that
            // intent (raises at runtime if the underlying record is
            // absent — same as Ruby's `NoMethodError on nil`).
            //
            // Excluded methods are the ones designed to operate on
            // nilable receivers — narrowing predicates (`nil?`,
            // `is_a?`, `kind_of?`, `instance_of?`, `respond_to?`) and
            // the Crystal-side null-safety helpers (`not_nil!`,
            // `try`, `inspect`, `to_s`). Without the exclusion, a
            // `parent.nil?` check turns into `parent.not_nil!.nil?`
            // — always false — and the surrounding `return if` is
            // dead code.
            let recv_s = if is_nilable_class_union(r.ty.as_ref())
                && is_simple_method_name(method.as_str())
                && !is_nil_safe_dispatch(method.as_str())
            {
                format!("{recv_s}.not_nil!")
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

/// True when `ty` is exactly a two-variant union of a `Ty::Class` and
/// `Ty::Nil` — the body-typer's canonical shape for a nilable-class
/// result (`ActiveRecord::Base#last` returns `Base?`, surfaces here
/// as `Union<Class, Nil>`). Drives the auto-`.not_nil!` recv rewrite
/// for chained Rails idioms (`Article.last.title`) that would
/// otherwise compile-fail under Crystal's strict null check.
fn is_nilable_class_union(ty: Option<&Ty>) -> bool {
    let Some(Ty::Union { variants }) = ty else { return false; };
    if variants.len() != 2 {
        return false;
    }
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    let has_class = variants.iter().any(|v| matches!(v, Ty::Class { .. }));
    has_nil && has_class
}

/// Method names that are part of Crystal's / Ruby's null-safety
/// surface and SHOULD NOT trigger an auto-`.not_nil!` recv rewrite.
/// Narrowing predicates (`nil?`, `is_a?`, …) and pass-through
/// helpers (`not_nil!`, `try`, `inspect`, `to_s`, `hash`) all
/// accept nilable receivers natively; pre-narrowing the recv would
/// either change semantics (`parent.not_nil!.nil?` is always false)
/// or be redundant.
fn is_nil_safe_dispatch(name: &str) -> bool {
    matches!(
        name,
        "nil?"
            | "is_a?"
            | "kind_of?"
            | "instance_of?"
            | "respond_to?"
            | "not_nil!"
            | "try"
            | "inspect"
            | "to_s"
            | "hash"
            | "object_id"
            | "==",
    )
}

/// True when a method name is a "simple" Ruby identifier — letters,
/// digits, underscores, optional trailing `?`/`!`. Excludes operator
/// methods (`[]`, `+`, `<<`, …) so the auto-`.not_nil!` rewrite stays
/// inside the bare-dispatch shape where the Rails idiom applies.
fn is_simple_method_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    let len = name.len();
    let body_end = if name.ends_with('?') || name.ends_with('!') || name.ends_with('=') {
        len - 1
    } else {
        len
    };
    name[..body_end]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        Literal::Int { value } => {
            // Crystal integer literals default to `Int32`. Roundhouse's
            // `Ty::Int` maps to `Int64` (Rails-style 64-bit IDs on
            // sqlite/MySQL — see `crystal_ty`); typed slots like
            // `@id : Int64?` and `@status : Int64?` therefore reject
            // a bare-literal `200` (Int32). Suffix every integer
            // literal with `_i64` so the literal type matches the
            // surrounding declared type. Crystal Int64 still
            // implicitly converts to `Int` (the abstract parent
            // accepted by `Array#[]`, `Hash#[]`, etc.), so call sites
            // expecting `Int32` parameters aren't affected.
            format!("{value}_i64")
        }
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
        LValue::Const { path } => path.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>().join("::"),
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
        Pattern::Expr { expr } => emit_expr(expr),
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
