//! Generic Go body/expression emission — used by the model method
//! emitter and by other modules that need a fallback for arbitrary
//! `Expr` rendering.
//!
//! Forked 2026-05-21 from the original go expr emitter so go2 can
//! evolve the walker independently (Phase 2+ type-aware emit,
//! lowered-IR coverage, transpiled-runtime call shapes).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::expr::{desugar_op_assign, Expr, ExprNode, IrHint, Literal};
use crate::ty::Ty;

use super::shared::{go_field_name, go_method_name};

/// Context threaded through the walker. Carries everything that's
/// context-sensitive — i.e. whose Go emit depends on the enclosing
/// class/method shape, not just the local Expr subtree.
///
/// `SelfRef` is the first variant that needs it: in a class method
/// (`def self.foo`), `self.bar(x)` emits as the bare-fn call
/// `ClassName_bar(x)`; in an instance method it emits as `self.Bar(x)`
/// against the Go `(self *ClassName)` receiver. Without ctx, the
/// walker has no way to know which to pick.
#[derive(Debug, Clone)]
pub(super) struct EmitCtx {
    /// Enclosing class name, already sanitized to a Go identifier
    /// (e.g. `JsonBuilder`, `ActiveRecordBase`, not the raw
    /// `ActiveRecord::Base`). `None` when emitting a module-mode
    /// bag of bare functions or outside any class.
    pub class_name: Option<String>,
    /// True when emitting inside a class method (`def self.foo`).
    /// Class methods in Go are bare functions named `Class_method`;
    /// `SelfRef` inside them refers to the class itself, NOT to a
    /// receiver parameter. Instance methods set this `false`.
    pub in_class_method: bool,
    /// Var name → replacement identifier. Used by the
    /// `is_a?(Class)` if-init rewrite: when `if _, ok := v.(string)`
    /// is rewritten to `if s, ok := v.(string)`, the then_branch
    /// emits with `v` → `s` so the asserted typed value is what
    /// nested call sites consume. Scoped to the child ctx; never
    /// applied to the else branch or the outer scope.
    pub var_renames: HashMap<String, String>,
    /// Names already declared in the enclosing method body —
    /// function params at entry, plus any `Var` lvalues assigned
    /// previously in source order. `emit_assign` picks `:=` on the
    /// first assignment to a name and `=` on subsequent ones,
    /// matching Ruby's flat-method-scope semantics (an inner Ruby
    /// `x = 1` reassigns the outer `x`; Go's `:=` would shadow,
    /// silently losing the write). Shared across child ctxs via
    /// `Rc<RefCell>` so the scope flattens — `with_rename` clones
    /// the Rc, not the contents.
    pub declared: Rc<RefCell<HashSet<String>>>,
    /// True when the enclosing method's signature returns `void`
    /// (RBS `() -> void` lowers to `Ty::Fn { ret: Ty::Nil, ... }`).
    /// emit_return_at suppresses the implicit `return X` wrap when
    /// set — Ruby methods have an implicit Lit::Nil tail that
    /// shouldn't emit as `return nil` against a Go void return type.
    pub void_method: bool,
    /// True when emitting a method on a Ruby module-singleton class
    /// (e.g. `module ActiveRecord; class << self; attr_accessor :adapter;
    /// end; end`). All methods of such a class have Class receiver, and
    /// every `@ivar` reference targets a per-slot package var
    /// (`<ClassName>_<ivar>_slot`) instead of an instance field or a
    /// bare module-level var. The library emit detects the shape and
    /// flips this flag; without it, Ivar reads/writes would either
    /// hit `self.Field` (no instance exists) or a bare `<ivar>` that
    /// collides across modules.
    pub in_module_singleton: bool,
    /// The enclosing method's declared return Ty. `emit_return_at`
    /// reads this to insert a Go conversion at `return X` when the
    /// inferred Go type of `X` doesn't match the function's declared
    /// return type. Specifically: Ty::Int returns wrap their value
    /// in `int64(...)` because Ruby `n = 0; ...; return n` lowers
    /// to `n := 0` (Go infers `int`, not `int64`) and bare `return n`
    /// against an `int64` signature fails. The redundant
    /// `int64(literal)` wrap is harmless for cases where it's not
    /// needed. `None` when no signature is set or the method is
    /// void.
    pub return_ty: Option<Ty>,
    /// Names of real (non-attr) instance methods on the enclosing
    /// class. Used to decide whether `self.foo` (0-arg implicit-self
    /// Send to a non-stdlib method) emits as a field read
    /// (`self.Foo`, no parens — the right shape for an attr_reader/
    /// writer-backed struct field) or a method call (`self.Foo()` —
    /// the right shape for a real method like `before_validation`).
    /// Stores raw Ruby names (`new_record?`, `valid?`, `before_save`)
    /// so the call-site lookup keys off the same string the IR
    /// carries. `None` outside a class body (module-mode bag of
    /// bare functions); the existing `is_known_go_method` stdlib
    /// whitelist still kicks in.
    pub self_methods: Option<Rc<HashSet<String>>>,
    /// True when the enclosing class is in the AR chain
    /// (transitively descends from `ActiveRecord::Base`). Drives the
    /// Q1 polymorphic-dispatch peepholes — `self.class.X` inside an
    /// AR-method body emits as `self.Self.X()` (interface dispatch
    /// through the back-pointer) rather than `<ClassName>_X()`
    /// (static class-method form), so subclass overrides land
    /// correctly. Same flag governs `self.OpSet/OpGet` routing.
    pub is_ar_class: bool,
    /// Go package name the surrounding file emits into. Phase 4.2
    /// cross-class-reference emit consults
    /// `super::paths::package_for_class(target)` to decide whether to
    /// qualify a reference; if `package_for_class(target) != current_pkg`,
    /// the emit prepends the target package and registers the import.
    /// Phase 1 invariant: every file is `"v2"`. Defaults to `"v2"` in
    /// `EmitCtx::none()` so existing call sites stay correct.
    pub current_pkg: &'static str,
}

impl EmitCtx {
    pub fn none() -> Self {
        Self {
            class_name: None,
            in_class_method: false,
            var_renames: HashMap::new(),
            declared: Rc::new(RefCell::new(HashSet::new())),
            void_method: false,
            in_module_singleton: false,
            self_methods: None,
            return_ty: None,
            is_ar_class: false,
            current_pkg: "v2",
        }
    }

    /// Seed the declared-names set with method parameter names so
    /// `Assign` to a param emits as `=` not `:=`. Idempotent.
    pub fn declare_param(&self, name: &str) {
        self.declared.borrow_mut().insert(name.to_string());
    }

    /// Build a child ctx with one extra Var rename pushed in. Used
    /// by the `If` handlers to expose the asserted typed value to
    /// the then_branch. Shares the declared-names set via Rc clone.
    pub fn with_rename(&self, from: String, to: String) -> Self {
        let mut child = self.clone();
        child.var_renames.insert(from, to);
        child
    }

    /// Enter a nested Go block scope (if-then, if-else, for-body,
    /// IIFE body). Snapshots the current `declared` names into a
    /// fresh `Rc<RefCell<HashSet>>` so that mutations inside the
    /// child scope don't leak back to the parent. Outer-scope vars
    /// stay visible (the snapshot starts with them populated), so
    /// inner reassigns of outer vars still correctly emit `=`. But
    /// inner FIRST-declarations of new vars (`v := ...`) only
    /// register in the child's set — sibling blocks each see a
    /// fresh scope and correctly emit `:=` for their own first
    /// declarations. Mirrors Go's lexical block scoping.
    pub fn enter_scope(&self) -> Self {
        let snapshot: HashSet<String> = self.declared.borrow().clone();
        let mut child = self.clone();
        child.declared = Rc::new(RefCell::new(snapshot));
        child
    }
}

pub(super) fn emit_expr(ctx: &EmitCtx, e: &Expr) -> String {
    // IrHint::StringBuilder* — lowerer-tagged accumulator triple
    // (`io = String.new; io << "..."; io`). Go's immutable strings
    // make `io = io + "..."` O(n²); swap to `strings.Builder` which
    // amortizes appends. Single hook at the entry of emit_expr so
    // all three sites — Assign, Send, terminal Var — rewrite in
    // one place. The "strings" import is auto-added by
    // `needed_imports` once `strings.Builder` appears in output.
    if let Some(s) = try_string_builder(ctx, e) {
        return s;
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => {
            // Substitute via ctx.var_renames if present — used by
            // the `is_a?` if-init rewrite to expose the asserted
            // typed value inside the then_branch. Otherwise sanitize
            // through the Go-keyword filter (`default` → `default_`)
            // so reads line up with the param-emit shape in
            // `library::render_params`.
            ctx.var_renames
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(|| super::library::sanitize(name.as_str()))
        }
        // `@field` in instance method bodies maps to `self.Field`
        // (the Go struct field synthesized by attr_reader/writer in
        // library.rs). In class methods (`def self.foo`), `@field`
        // refers to module-singleton state — emit as a bare
        // lowercase name that resolves to the package-level `var`
        // generated by `format_module_ivar`. When the class is a
        // module-singleton (`is_module=true` + every method Class
        // receiver), the package var is namespaced by class name
        // (`<Class>_<ivar>_slot`) to avoid collisions across modules
        // that happen to share an ivar name (`@adapter` on two
        // unrelated modules would otherwise both bind to `var adapter`).
        ExprNode::Ivar { name } => {
            if ctx.in_module_singleton {
                if let Some(class) = ctx.class_name.as_deref() {
                    format!("{class}_{}_slot", name.as_str())
                } else {
                    name.as_str().to_string()
                }
            } else if ctx.in_class_method {
                name.as_str().to_string()
            } else {
                format!("self.{}", go_field_name(name.as_str()))
            }
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_send(ctx, recv.as_ref(), method.as_str(), args, block.as_ref(), e.ty.as_ref())
        }
        // `self` reference. In an instance method, the Go body has
        // `(self *Class)` and `self` is a valid identifier — emit
        // verbatim. In a class method, there's no `self` parameter;
        // bare `SelfRef` would compile to a dangling identifier.
        // Emit the sanitized class name as a stand-in (it parses as
        // a Go reference to the type itself, surfacing the gap if
        // the surrounding context expects an instance). Without a
        // class name in ctx (module-mode), emit a TODO marker.
        ExprNode::SelfRef => self_ref_expr(ctx),
        // Ruby `super` in a model `initialize` (the only shape real-blog
        // emits): its effect — running the parent's constructor — is
        // already performed by go2's struct-embedding init
        // (`&T{ApplicationRecord: NewApplicationRecord(...)}`), so there
        // is nothing to emit. Render a documenting no-op comment rather
        // than falling through to the unsupported catch-all (which would
        // panic mid-constructor and leave the rest unreachable). General
        // `super` to a non-ctor parent method isn't wired yet; if one
        // ever appears it still no-ops here — acceptable parity with the
        // pre-#28 drop, and the catch-all still guards every other node.
        ExprNode::Super { .. } => {
            "// super: parent dispatch handled via struct embedding".to_string()
        }
        // Ruby `x = value` — legacy go drops the lvalue and emits
        // only the rhs, which loses the binding. go2 needs the real
        // assignment so subsequent statements can refer to `x`.
        ExprNode::Assign { target, value } => emit_assign(ctx, target, value),
        // Go has native `+=`, `-=`, etc. but no `||=`/`&&=`. Desugar
        // to existing IR forms (`target = target op value` for
        // arithmetic; conditional Assign for short-circuit) and let
        // the existing emit machinery render. The Rails dirty-tracking
        // concern that motivated the IR variant is Ruby-runtime
        // specific — Go has no equivalent, so the desugar is faithful
        // for this target.
        ExprNode::OpAssign { target, op, value } => {
            let desugared = desugar_op_assign(target, *op, value, e.span);
            emit_expr(ctx, &desugared)
        }
        // Ruby `return X` — emits as a Go return statement. In Ruby
        // this is technically an Expr (type `Never`), so it can
        // appear in value position; the body-position walker
        // (`emit_return_body`) intercepts most uses. The remaining
        // path is `return X if cond` lowered to `If { then: Return,
        // else: Nil }` inside a `Seq`; legacy `emit_expr` for If
        // walks branches via `emit_block_body`, so the Return ends
        // up emitted as a statement inside a Go block — valid.
        ExprNode::Return { value } => {
            // Ruby `return if cond` lowers to `Return { Lit::Nil }`.
            // In a truly void Go function (return_ty == Ty::Nil),
            // `return nil` is invalid ("too many return values");
            // emit bare `return`. Use `ctx.return_ty` rather than
            // `ctx.void_method` — the latter is also flipped by
            // emit_constructor for statement-shape body emit, but
            // the constructor's actual return type is `*Class` and
            // `return nil` against a nilable pointer remains correct.
            let truly_void = matches!(ctx.return_ty.as_ref(), Some(Ty::Nil));
            if truly_void && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                "return".to_string()
            } else {
                format!("return {}", emit_expr(ctx, value))
            }
        }
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|sub| emit_expr(ctx, sub))
            .collect::<Vec<_>>()
            .join("; "),
        ExprNode::While { cond, body, until_form } => {
            // Ruby `while cond ... end` → Go `for cond { ... }`.
            // `until cond` (the `until_form` flag) negates the cond:
            // `for !(cond) { ... }`. Ruby `while` evaluates to nil;
            // Go's `for` is a statement. emit_return_at's `_ =>`
            // fallback wraps the while value as `return …`, which
            // is invalid — body-position while loops will need a
            // dedicated emit_return_at::While arm (deferred until
            // a body actually puts a while at tail position).
            let cond_s = emit_expr(ctx, cond);
            let cond_text = if *until_form {
                format!("!({cond_s})")
            } else {
                cond_s
            };
            // Loop body is a Go block — fresh declared scope so
            // `v := ...` inside the loop doesn't bleed to siblings.
            let body_s = emit_block_body(&ctx.enter_scope(), body);
            format!("for {cond_text} {{\n{body_s}\n}}")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // `unless cond ... end` lowers to `If { then: Lit::Nil,
            // else: ... }`. Invert before emit so we don't produce
            // an invalid bare-nil then-block.
            if is_nil_lit(then_branch) && !is_nil_lit(else_branch) {
                let cond_s = emit_expr(ctx, cond);
                let else_s = emit_block_body(&ctx.enter_scope(), else_branch);
                return format!("if !({cond_s}) {{\n{else_s}\n}}");
            }
            // `if recv.is_a?(Class)` → Go's type-assert init form
            // `if asserted, ok := recv.(GoTy); ok`. The then_branch
            // gets a child ctx that renames the recv's Var to the
            // asserted ident, so nested uses see the typed value.
            // Both branches enter fresh scopes — sibling-block `:=`
            // declarations stay isolated.
            let (init, cond_s, then_ctx) = match try_emit_is_a_init(ctx, cond) {
                Some(IsAInit { init, cond, recv_name, asserted_ident }) => {
                    let child = match recv_name {
                        Some(n) => ctx.enter_scope().with_rename(n, asserted_ident.to_string()),
                        None => ctx.enter_scope(),
                    };
                    (init, cond.to_string(), child)
                }
                None => (String::new(), emit_expr(ctx, cond), ctx.enter_scope()),
            };
            let then_s = emit_block_body(&then_ctx, then_branch);
            // `return X if cond` lowers to If { else: Lit::Nil } —
            // emit without the else clause so the body parses as
            // valid Go (a bare `nil` statement is invalid).
            if is_nil_lit(else_branch) {
                format!("if {init}{cond_s} {{\n{then_s}\n}}")
            } else {
                let else_s = emit_block_body(&ctx.enter_scope(), else_branch);
                format!("if {init}{cond_s} {{\n{then_s}\n}} else {{\n{else_s}\n}}")
            }
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(ctx, k), emit_expr(ctx, v)))
                .collect();
            // Prefer the analyzer-set Ty on this Expr when present —
            // it represents the declared/inferred type at this
            // position and outranks the literal-inspecting heuristic
            // below (which has no access to the surrounding context).
            // Either side mapping to `interface{}` (Untyped/Var/
            // Bottom/Record/Tuple via `go_ty_stub`) still carries
            // signal — `Hash[Symbol, Untyped]` declares the value
            // type as untyped, and emitting `map[string]interface{}`
            // matches that declaration. The heuristic stays as the
            // no-Ty fallback for emit shapes outside method-return
            // position (e.g. inline literal expressions whose Ty
            // wasn't propagated by the analyzer).
            if let Some(Ty::Hash { key, value }) = e.ty.as_ref() {
                let k_ty = super::ty::go_ty_stub(Some(key));
                let v_ty = super::ty::go_ty_stub(Some(value));
                // Fire when at least one side maps to a concrete Go
                // type. `Hash[Var, Var]` (unresolved by analyzer) and
                // `Hash[Untyped, Untyped]` (declared catchall) both
                // resolve to interface{}/interface{} — those carry
                // less signal than the heuristic below, which assumes
                // string keys (the dominant Ruby Hash shape). Fire
                // when EITHER side is concrete so `Hash[Sym, Untyped]`
                // pins the key and lets the heuristic-equivalent
                // value default kick in.
                if k_ty != "interface{}" || v_ty != "interface{}" {
                    return format!("map[{k_ty}]{v_ty}{{{}}}", parts.join(", "));
                }
            }
            // Fallback: infer concrete element types from the
            // literal shape. Constants (`STATUS_CODES = { ok: 200,
            // ... }`) parse outside the body-typer's reach, so
            // their Hash node lands here with e.ty == None and the
            // value type has to be derived from the literals
            // themselves. When EVERY value is the same primitive
            // literal kind (Str/Int/Float/Bool), pin that as the
            // value type — `map[string]int64` for STATUS_CODES,
            // `map[string]string` for HTML_ESCAPES. Mixed-kind
            // and empty fall back to `map[string]interface{}`.
            let key_kind = literal_kind_str("string");
            let val_kind = uniform_value_literal_kind(entries);
            let (k_ty, v_ty) = match (key_kind, val_kind) {
                (k, Some(v)) => (k, v),
                _ => ("string", "interface{}"),
            };
            format!("map[{k_ty}]{v_ty}{{{}}}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(ctx, e)).collect();
            // Prefer the analyzer's Ty when it maps to a concrete Go
            // elem (not the catch-all `interface{}` for Untyped/Var/
            // etc.). Empty `[]` literals against a typed-field
            // destination (`@errors: Array[String]`) pin the elem
            // here; literals with no surrounding type info land at
            // `Array[Untyped]` and fall through to the bare default.
            if let Some(Ty::Array { elem }) = e.ty.as_ref() {
                let elem_ty = super::ty::go_ty_stub(Some(elem));
                if elem_ty != "interface{}" {
                    return format!("[]{elem_ty}{{{}}}", parts.join(", "));
                }
            }
            format!("[]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            // Ruby `||` returns the first truthy operand; Go's `||`
            // requires bool operands. For non-bool operand types
            // (`slots[k] || ""`, `attrs["id"] || 0`, …), use `cmp.Or`
            // (Go 1.22+) which returns the first non-zero value.
            // Trigger when EITHER side is a known primitive (Str, Sym,
            // Int, Float). Both-Bool falls through to Go's bool `||`.
            // Both-Untyped also falls through (no signal that either
            // is meant numerically); the catch-all `||` emit there
            // still produces invalid Go for now but the case isn't
            // observed in framework Ruby today.
            if matches!(op, BoolOpKind::Or) {
                let primitive_kind = |t: &Option<Ty>| {
                    matches!(t, Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float))
                };
                if primitive_kind(&left.ty) || primitive_kind(&right.ty) {
                    return format!(
                        "cmp.Or({}, {})",
                        emit_expr(ctx, left),
                        emit_expr(ctx, right)
                    );
                }
            }
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(ctx, left), op_s, emit_expr(ctx, right))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut fmt = String::new();
            let mut args: Vec<String> = Vec::new();
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '%' {
                                fmt.push_str("%%");
                            } else {
                                fmt.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        fmt.push_str("%v");
                        args.push(emit_expr(ctx, expr));
                    }
                }
            }
            if args.is_empty() {
                go_str_literal(&fmt)
            } else {
                format!("fmt.Sprintf({}, {})", go_str_literal(&fmt), args.join(", "))
            }
        }
        // Ruby `yield args` — invokes the caller-provided block.
        // Go has no native block-yield; the idiomatic Go shape is a
        // closure parameter the caller passes. Until that shape lands
        // (which would change every yielding method's signature),
        // emit as a panic so the file parses and the gap surfaces
        // loudly at runtime. The args get `_ = ...` references so
        // surrounding `v := self.Data[k]; yield k, v` doesn't leave
        // `v` as an unused-local vet error.
        ExprNode::Yield { args } => {
            let arg_uses: Vec<String> = args
                .iter()
                .map(|a| format!("\t_ = {}", emit_expr(ctx, a)))
                .collect();
            let body = if arg_uses.is_empty() {
                String::new()
            } else {
                format!("{}\n", arg_uses.join("\n"))
            };
            format!(
                "func() interface{{}} {{\n\
                 {body}\
                 \tpanic(\"yield not implemented in go2 emit\")\n\
                 }}()",
            )
        }
        ExprNode::Cast { value, target_ty } => emit_cast(ctx, value, target_ty),
        ExprNode::Case { scrutinee, arms } => emit_case(ctx, scrutinee, arms, e.ty.as_ref()),
        // Ruby `next` inside a block → Go `continue`. The `each`/`map`
        // block emit wraps the body in a real `for … range` loop (not a
        // nested IIFE), so a value-less `next` lands directly inside the
        // loop and `continue` is valid Go. The value-carrying form
        // (`next x` in a `map`/`select` accumulator) has no statement-level
        // Go equivalent here, so it stays unsupported and surfaces loudly.
        ExprNode::Next { value } => match value {
            None => "continue".to_string(),
            Some(_) => {
                crate::emit::diagnostics::report_unsupported(e.span, "go2", "Next", "value-carrying")
            }
        },
        other => crate::emit::diagnostics::report_unsupported(e.span, "go2", other.kind_str(), ""),
    }
}

/// Emit `case scrutinee; when Pat then body; …; end` as an IIFE
/// wrapping a Go `switch`. The lowerer's per-column dispatch in
/// model `[]/[]=` (Article#OpGet / Article#OpSet) is the primary
/// trigger — `case name when :id then @id when :body then @body end`.
/// IIFE shape rather than bare switch so the result can flow into
/// any expression position (return tail, RHS of assign, arg slot);
/// emit_return_body's surrounding `return X` wrap composes with the
/// IIFE's own returns.
///
/// Pattern coverage:
/// - `Pattern::Lit { value }` → `case <go_literal>:` (Sym/Str both
///   compare against Go strings; Int → numeric switch case)
/// - `Pattern::Wildcard` → `default:`
/// - `Pattern::Bind { name }` (Ruby `else` with a binding) → `default:`,
///   the bind isn't surfaced into Go since the scrutinee is in scope
///   already and Ruby's bind name is rarely referenced.
/// - Array/Record patterns fall through to a TODO marker; the lowerer
///   doesn't currently emit these for model dispatch.
///
/// Trailing `return <zero>` after the switch keeps Go's flow analysis
/// happy — even with a `default` arm, Go doesn't infer switch
/// exhaustiveness across all literal values.
fn emit_case(
    ctx: &EmitCtx,
    scrutinee: &Expr,
    arms: &[crate::expr::Arm],
    result_ty: Option<&Ty>,
) -> String {
    let scrutinee_s = emit_expr(ctx, scrutinee);
    let go_ret = super::ty::go_ty_stub(result_ty);
    let mut out = format!("func() {go_ret} {{\n\tswitch {scrutinee_s} {{\n");
    // Use the tail-aware return emitter for each arm body so Assign
    // tails (Article#OpSet's `case :id then @id = value` shape) emit
    // as `self.ID = converted; return self.ID` rather than the
    // invalid `return self.ID = converted` (Go doesn't allow assign
    // as an expression). Each arm gets a fresh ctx scope so sibling
    // `:=` declarations don't leak across cases.
    for arm in arms {
        let arm_ctx = ctx.enter_scope();
        let body_s = emit_return_body(&arm_ctx, &arm.body);
        match &arm.pattern {
            crate::expr::Pattern::Lit { value } => {
                let pat_s = emit_literal(value);
                out.push_str(&format!("\tcase {pat_s}:\n{body_s}"));
            }
            crate::expr::Pattern::Wildcard | crate::expr::Pattern::Bind { .. } => {
                out.push_str(&format!("\tdefault:\n{body_s}"));
            }
            _ => {
                out.push_str(&format!(
                    "\t/* TODO: case pattern {:?} */\n",
                    std::mem::discriminant(&arm.pattern)
                ));
            }
        }
    }
    out.push_str("\t}\n");
    out.push_str(&format!("\treturn {}\n}}()", go_case_zero(result_ty)));
    out
}

/// Trailing-return fallback for `emit_case`'s IIFE — Go's flow
/// String-accumulator hint consumer. The lowerer (view_to_library /
/// jbuilder_to_library) tags the three sites of its `io = String.new;
/// io << "..."; io` pattern with `IrHint::StringBuilder*`; this
/// helper rewrites them to Go's `strings.Builder` idiom so the inner
/// concat stays O(n) instead of O(n²).
///
/// - `Init` rewrites the whole `Assign { Var, String.new }` to
///   `var <name> strings.Builder`, bypassing the value-side emit
///   (which would otherwise produce the `String.new → ""` peephole
///   plus an `:=` declaration).
/// - `Append` rewrites the `<name> << <arg>` Send to
///   `<name>.WriteString(<arg>)`, bypassing the otherwise-applied
///   `<name> = <name> + <arg>` rewrite for Str-typed recv.
/// - `Result` rewrites the terminal Var to `<name>.String()`, so
///   the function returns the finalized string.
///
/// Safe-by-construction local retype: every `<name>` reference in
/// the lowered body flows through one of the three tagged sites
/// (Init synthesizes once; every append goes through
/// `accumulator_append_call`; terminal ref via
/// `accumulator_result_ref`). Nothing else reads `io`, so the
/// emit-side type change from `string` to `strings.Builder` is
/// invisible to surrounding code.
fn try_string_builder(ctx: &EmitCtx, e: &Expr) -> Option<String> {
    use crate::expr::LValue;
    match e.hint? {
        IrHint::StringBuilderInit => {
            if let ExprNode::Assign {
                target: LValue::Var { name, .. }, ..
            } = &*e.node
            {
                let var = super::library::sanitize(name.as_str());
                return Some(format!("var {var} strings.Builder"));
            }
            None
        }
        IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        let var = super::library::sanitize(name.as_str());
                        // A coalesced view append (`io << "…#{a}…#{b}…"`)
                        // arrives as a StringInterp. Collapsing it to one
                        // `WriteString(fmt.Sprintf("…%v…", a, b))` forces
                        // every fragment — including already-`Str`-typed
                        // ones — through the reflection-based `%v`
                        // formatter and boxes each into `interface{}`.
                        // Split into one `WriteString` per part instead:
                        // literal text and `Str`-typed expressions write
                        // directly (no Sprintf, no boxing); only genuinely
                        // non-string values keep a `Sprintf("%v", …)`. The
                        // emitted bytes are identical — same fragments,
                        // same order — so `compare go` is unaffected.
                        if let ExprNode::StringInterp { parts } = &*args[0].node {
                            return Some(emit_interp_appends(ctx, &var, parts));
                        }
                        let arg = emit_expr(ctx, &args[0]);
                        return Some(format!("{var}.WriteString({arg})"));
                    }
                }
            }
            None
        }
        IrHint::StringBuilderResult => {
            if let ExprNode::Var { name, .. } = &*e.node {
                let var = super::library::sanitize(name.as_str());
                return Some(format!("{var}.String()"));
            }
            None
        }
    }
}

/// Emit a coalesced view append as a sequence of `<var>.WriteString(…)`
/// statements, one per `StringInterp` part. Literal text and
/// `Str`-typed expressions write directly; non-string parts retain a
/// `fmt.Sprintf("%v", …)` so their stringification (and the emitted
/// bytes) match the prior single-Sprintf shape. Returned newline-joined
/// — only ever called in statement position (the append hint sits on a
/// body statement), so multiple lines are well-formed.
fn emit_interp_appends(ctx: &EmitCtx, var: &str, parts: &[crate::expr::InterpPart]) -> String {
    use crate::expr::InterpPart;
    let mut lines: Vec<String> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                lines.push(format!("{var}.WriteString({})", go_str_literal(value)));
            }
            InterpPart::Expr { expr } => {
                let s = emit_expr(ctx, expr);
                if matches!(expr.ty, Some(Ty::Str)) {
                    lines.push(format!("{var}.WriteString({s})"));
                } else {
                    lines.push(format!("{var}.WriteString(fmt.Sprintf(\"%v\", {s}))"));
                }
            }
        }
    }
    // Indentation is cosmetic only (Go ignores it; the emitted source
    // isn't gofmt'd). The top-level Seq emitter indents just the first
    // line of a statement, so prefix continuation lines with one tab to
    // line up the common case (a body-level append run). Inside a
    // re-indented block the continuations end up one tab deeper than the
    // first line — a minor wart we accept rather than thread depth
    // through every leaf emitter.
    lines.join("\n\t")
}

/// analysis requires a return after the switch even when every arm
/// returns. `interface{}` slot → `nil`; primitives → typed zero.
fn go_case_zero(ty: Option<&Ty>) -> &'static str {
    match ty {
        Some(Ty::Int) => "0",
        Some(Ty::Float) => "0.0",
        Some(Ty::Bool) => "false",
        Some(Ty::Str) | Some(Ty::Sym) => "\"\"",
        _ => "nil",
    }
}

pub(super) fn emit_send(
    ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    result_ty: Option<&Ty>,
) -> String {
    // Render kwarg expansion peephole. Ruby `render body,
    // content_type: "json"` lowers to a Send with args
    // `[body, Hash{content_type: ...}]`. The promoted
    // `(*ActionControllerBase).Render(body, status, content_type,
    // location string)` ctor takes 4 positional strings — without
    // kwarg expansion the call vet-errors as arity mismatch. Pad here:
    // - body comes from args[0]
    // - status/content_type/location come from the trailing-Hash's
    //   matching key (if present) or the empty-string default
    // - status defaults to "ok" to match the Ruby `status: :ok` source
    // The result re-routes through the standard `{}.{}({})` emit
    // below with the padded args list.
    let render_padded = try_expand_render_kwargs(ctx, recv, method, args)
        .or_else(|| try_expand_redirect_to_kwargs(ctx, recv, method, args))
        .or_else(|| try_expand_head_kwargs(ctx, recv, method, args))
        .or_else(|| try_expand_truncate_kwargs(ctx, recv, method, args));

    // Hash-widening peephole for module-singleton class methods whose
    // hand-written shim takes `map[string]interface{}` but whose call
    // site passes a uniform-string Hash literal (analyzer pins as
    // `Hash[Str, Str]`, emit lands as `map[string]string`). The
    // ty_coerce_insertion lowerer doesn't fire because (a) hand-written
    // shims (Broadcasts) aren't in its LibraryClass registry and
    // (b) the seeded sig uses `KeywordRest` which the lowerer filters.
    //
    // Widen here by retyping the arg Hash node before emit — set
    // `ty = Hash[Str, Untyped]` so the analyzer-Ty branch in
    // `ExprNode::Hash` emit produces `map[string]interface{}{…}`.
    // Restricted to a small allow-list of known kwargs-shim callees so
    // the rewrite doesn't accidentally widen genuine narrow-Hash
    // dispatches.
    let widen_hash_args = is_broadcasts_call(recv, method) || is_view_helpers_widen_call(recv, method);
    let args_s: Vec<String> = if let Some(padded) = render_padded {
        padded
    } else {
        args
            .iter()
            .map(|a| {
                // `recv[range]` slice indexing (handled in the `method ==
                // "[]"` block below) re-derives Go slice syntax straight
                // from the raw Range node and returns before args_s is
                // read for that arg. Emitting the Range here as a value
                // would hit the unsupported-Range fallthrough and push a
                // spurious diagnostic, so skip it — the placeholder is
                // never used.
                if method == "[]" {
                    if let ExprNode::Range { .. } = &*a.node {
                        return String::new();
                    }
                }
                if widen_hash_args {
                    if let ExprNode::Hash { .. } = &*a.node {
                        let mut widened = a.clone();
                        widened.ty = Some(Ty::Hash {
                            key: Box::new(Ty::Str),
                            value: Box::new(Ty::Untyped),
                        });
                        return emit_expr(ctx, &widened);
                    }
                }
                emit_expr_as_value(ctx, a)
            })
            .collect()
    };

    // Inline `ViewHelpers.dom_id(record [, suffix])` at the call
    // site when `record`'s Ty is a concrete model (Article, Comment,
    // ...). The runtime `dom_id(record *ActiveRecordBase, ...)`
    // takes the abstract base struct, and Go has no implicit upcast
    // from `*Article` to `*ActiveRecordBase` — every call site
    // would need `&article.ActiveRecordBase`, ugly and fragile.
    // Mirrors rust2's `try_view_helpers_dom_id` (src/emit/rust2/
    // expr/send/mod.rs:322); the Ruby body is a one-line format,
    // so the inline is direct.
    if let Some(s) = try_dom_id_inline(recv, method, args, &args_s) {
        return s;
    }

    // Ruby `raise X, "msg"` / `raise "msg"` / `raise X` — parses as
    // an implicit-self Send with method `raise`. Maps to Go
    // `panic(<typed_value>)`. When the class arg is one of the known
    // framework exception classes (RecordNotFound, NotImplementedError,
    // RecordInvalid), wrap the message in a typed sentinel struct so
    // the HTTP router_glue's defer-recover can type-switch and map
    // RecordNotFound to a 404 response (matching Rails' rescue-and-
    // serve behavior). Unknown classes still emit a bare-string panic
    // — the message preserves diagnostic context for un-recovered
    // panics. 0-arg bare `raise` (Ruby re-raise) emits a placeholder
    // panic since Go has no current-exception slot. The tail-position
    // return wrap is suppressed in `emit_return_at` so `return
    // panic(...)` (a syntax error) never lands.
    if recv.is_none() && method == "raise" {
        let msg = match args_s.len() {
            0 => "\"raise\"".to_string(),
            1 => args_s[0].clone(),
            _ => args_s[args_s.len() - 1].clone(),
        };
        // Detect typed-class raise: first arg is a Const naming a
        // known framework error class. Wrap the message in a Go
        // sentinel struct (defined in runtime/go/v2/errors.go).
        if args.len() >= 2 {
            if let ExprNode::Const { path } = &*args[0].node {
                if let Some(last) = path.last() {
                    let sentinel = match last.as_str() {
                        "RecordNotFound" => Some("RecordNotFoundError"),
                        "NotImplementedError" => Some("NotImplementedErrorValue"),
                        "RecordInvalid" => Some("RecordInvalidError"),
                        _ => None,
                    };
                    if let Some(struct_name) = sentinel {
                        return format!("panic(&{struct_name}{{Message: {msg}}})");
                    }
                }
            }
        }
        return format!("panic({msg})");
    }

    // Bare `name` inside a class method — Ruby's "self.name" when
    // self is a class returns the class's string name. In
    // `def self.schema_columns; raise NotImplementedError,
    //  "#{name}.schema_columns must be overridden"; end` the
    // interpolated `name` is exactly this. Go has no class object;
    // resolve at emit time to a string literal of the enclosing
    // class name. Subclass overrides won't reroute this lookup
    // (it's frozen to the defining class), but the Base body that
    // uses it is the NotImplementedError raise — a diagnostic
    // string, not a dispatch path. Restricted to bare implicit-self
    // (`recv.is_none()`) so user-defined `.name` methods on
    // instances still route through normal dispatch.
    if recv.is_none()
        && method == "name"
        && args.is_empty()
        && ctx.in_class_method
    {
        if let Some(class_name) = ctx.class_name.as_deref() {
            return format!("{class_name:?}");
        }
    }

    // `recv._go_try_fetch(k) { |v| body }` — synthesized by
    // `lower::nil_check_to_comma_ok`, NOT a real Ruby method. Emit
    // as the Go comma-ok idiom inside an IIFE so the local `v` + `ok`
    // bindings don't leak into the surrounding scope. The IIFE
    // returns nothing — the body is statement-shaped (assignments,
    // calls), not expression-shaped.
    if method == super::lower::nil_check_to_comma_ok::SENTINEL_METHOD
        && args.len() == 1
        && recv.is_some()
    {
        if let Some(block_e) = block {
            if let ExprNode::Lambda { params, body, .. } = &*block_e.node {
                if params.len() == 1 {
                    let recv_s = emit_expr(ctx, recv.unwrap());
                    let key_s = &args_s[0];
                    let var_name = super::library::sanitize(params[0].as_str());
                    // IIFE introduces a fresh Go scope — clone declared.
                    let body_ctx = ctx.enter_scope();
                    body_ctx.declare_param(params[0].as_str());
                    let body_s = emit_block_body(&body_ctx, body);
                    return format!(
                        "func() {{\n\
                         \tif {var_name}, ok := {recv_s}[{key_s}]; ok {{\n\
                         \t\t_ = {var_name}; _ = ok\n\
                         {body_s}\n\
                         \t}}\n\
                         }}()",
                    );
                }
            }
        }
    }

    // `recv.each { |x| body }` (1-param) and `recv.each { |k, v| body }`
    // (2-param) → Go `for ... range` loop wrapped in an IIFE that
    // returns the receiver (Ruby `each` semantics). The IIFE wrap
    // makes the statement-shaped loop fit anywhere an expression
    // goes — assignment value, Seq middle, method tail. Return type
    // `interface{}` keeps the wrap total without needing receiver-
    // type inference at this emit site; callers that discard the
    // value (`Seq` middle, `each` in void-method tail) silently
    // drop it, matching Go's "discarded return is fine" semantics.
    if method == "each" && args.is_empty() {
        if let (Some(recv_e), Some(block_e)) = (recv, block) {
            if let ExprNode::Lambda { params, body, .. } = &*block_e.node {
                let raw_recv_s = emit_expr(ctx, recv_e);
                // If recv resolves to `interface{}` (Untyped/Var/
                // Bottom from go_ty_stub), Go can't range over it
                // directly. The block's arity tells us the shape:
                // 2 params ⇒ Hash, so assert to `map[string]any`;
                // 0/1 params ⇒ Array, assert to `[]any`. The
                // assertion runtime-panics if recv is actually the
                // wrong shape, which mirrors Ruby's NoMethodError
                // on `.each` against a non-enumerable.
                let recv_s = if recv_ty_renders_as_interface(recv_e) {
                    match params.len() {
                        2 => format!("{raw_recv_s}.(map[string]any)"),
                        _ => format!("{raw_recv_s}.([]any)"),
                    }
                } else {
                    raw_recv_s
                };
                // Loop vars: 1 param → array iter (drop the index
                // with `_`); 2 params → hash iter (key + value);
                // 0 params (rare — `arr.each { puts "hi" }`) → both
                // sides bound to `_`. >2 params is unmappable; emit
                // a TODO so the gap is loudly visible.
                // IIFE introduces fresh Go scope.
                let body_ctx = ctx.enter_scope();
                let range_vars = match params.len() {
                    0 => "_, _".to_string(),
                    1 => {
                        let name = params[0].as_str();
                        body_ctx.declare_param(name);
                        format!("_, {name}")
                    }
                    2 => {
                        let k = params[0].as_str();
                        let v = params[1].as_str();
                        body_ctx.declare_param(k);
                        body_ctx.declare_param(v);
                        format!("{k}, {v}")
                    }
                    _ => {
                        return format!(
                            "/* TODO: each block with {} params */",
                            params.len(),
                        );
                    }
                };
                let body_s = emit_block_body(&body_ctx, body);
                // 2-param `each` iterates a Hash. Go map iteration order is
                // undefined per-run, but several runtime-Ruby surfaces
                // render observable output from it — notably `render_attrs`,
                // whose HTML attribute order feeds Turbo's
                // `data-turbo-track="reload"` element comparison. An
                // unstable order makes Turbo think tracked assets changed on
                // every navigation and forces a full page reload, defeating
                // Drive (and breaking the e2e turbo/cable specs). Iterate
                // sorted keys for a stable order. Sorted (not insertion)
                // order differs from Rails, but `compare`'s DOM diff stores
                // attrs in a BTreeMap and is attribute-order-insensitive, so
                // byte-parity is unaffected. Only string-keyed maps take this
                // path (the common case); other key types keep the unordered
                // range. Binding `_m` also avoids the double-eval of a
                // non-trivial `recv_s` the unordered form has.
                if params.len() == 2 {
                    let (k_ty, _v_ty) = hash_kv_go_tys(recv_e.ty.as_ref());
                    if k_ty == "string" {
                        let k = params[0].as_str();
                        let v = params[1].as_str();
                        return format!(
                            "func() interface{{}} {{\n\
                             \t_m := {recv_s}\n\
                             \t_ks := make([]string, 0, len(_m))\n\
                             \tfor _k := range _m {{ _ks = append(_ks, _k) }}\n\
                             \tsort.Strings(_ks)\n\
                             \tfor _, {k} := range _ks {{\n\
                             \t\t{v} := _m[{k}]\n\
                             \t\t_ = {v}\n\
                             {body_s}\n\
                             \t}}\n\
                             \treturn _m\n\
                             }}()",
                        );
                    }
                }
                return format!(
                    "func() interface{{}} {{\n\
                     \tfor {range_vars} := range {recv_s} {{\n\
                     {body_s}\n\
                     \t}}\n\
                     \treturn {recv_s}\n\
                     }}()",
                );
            }
        }
    }

    // `recv.map { |x| body }` (1-param) → Go IIFE that builds a new
    // slice by iterating the receiver and appending each body's tail
    // value. Mirrors `each` shape but accumulates instead of
    // discarding. The accumulator's element type comes from the
    // analyzer-set `result_ty` (`Array[Base]` → `[]*ActiveRecordBase`)
    // when it pins to a concrete Go elem; otherwise falls back to
    // `[]interface{}`. Mirrors the literal Ty back-prop landed in
    // 8d9f06d but for the IIFE result instead of an Array literal.
    if method == "map" && args.is_empty() {
        if let (Some(recv_e), Some(block_e)) = (recv, block) {
            if let ExprNode::Lambda { params, body, .. } = &*block_e.node {
                let recv_s = emit_expr(ctx, recv_e);
                // IIFE introduces fresh Go scope.
                let body_ctx = ctx.enter_scope();
                let range_vars = match params.len() {
                    0 => "_, _".to_string(),
                    1 => {
                        let name = params[0].as_str();
                        body_ctx.declare_param(name);
                        format!("_, {name}")
                    }
                    2 => {
                        let k = params[0].as_str();
                        let v = params[1].as_str();
                        body_ctx.declare_param(k);
                        body_ctx.declare_param(v);
                        format!("{k}, {v}")
                    }
                    _ => {
                        return format!(
                            "/* TODO: map block with {} params */",
                            params.len(),
                        );
                    }
                };
                let body_s = emit_map_block_body(&body_ctx, body);
                let elem_ty = match result_ty {
                    Some(Ty::Array { elem }) => {
                        let rendered = super::ty::go_ty_stub(Some(elem));
                        if rendered == "interface{}" {
                            "interface{}".to_string()
                        } else {
                            rendered
                        }
                    }
                    _ => "interface{}".to_string(),
                };
                return format!(
                    "func() []{elem_ty} {{\n\
                     \tout := []{elem_ty}{{}}\n\
                     \tfor {range_vars} := range {recv_s} {{\n\
                     {body_s}\n\
                     \t}}\n\
                     \treturn out\n\
                     }}()",
                );
            }
        }
    }

    // `Time.now.utc.iso8601` → `time.Now().UTC().Format(time.RFC3339)`.
    // The chain has no element-wise Go analog (no `Time` class method,
    // no chained `.utc` on the result, no `.iso8601` formatter). Match
    // the full three-step chain at the outermost Send and emit the Go
    // equivalent in one shot. Partial chains (`Time.now` alone,
    // `Time.now.utc`) aren't hit by the runtime/ruby/ surface today;
    // those would still fall through to the generic Const-recv
    // class-method fallback and emit `Time_now()` (undefined) so the
    // gap stays loud. The literal `time.RFC3339` triggers the `time`
    // import via `super::needed_imports`.
    if method == "iso8601" && args.is_empty() {
        if is_time_now_utc_chain(recv) {
            return "time.Now().UTC().Format(time.RFC3339)".to_string();
        }
    }

    // Ruby `recv.field = v` lowers in the IR as `Send { recv, method:
    // "field=", args: [v] }`. Emit as Go field assignment
    // `recv.Field = v` rather than method-call shape `recv.Field=(v)`
    // (which Go parses as a method named `Field` with `=(v)` operand
    // — invalid). Special-case the `[]=` operator method below routes
    // through OpSet; ordinary writers land here. The `=` suffix is
    // peeled before `go_field_name` so the "id" → "ID" special case
    // fires (without the peel, "id=" pascalizes to "Id=" and breaks
    // the Go field lookup).
    if is_writer_method_name(method) && args.len() == 1 && recv.is_some() {
        let r = recv.unwrap();
        let recv_s = emit_expr(ctx, r);
        let field_ruby = &method[..method.len() - 1];
        let field_go = go_field_name(field_ruby);
        // If/Case-shaped RHS needs IIFE wrap — Go's `if`/`switch` are
        // statements, not expressions, so `instance.X = if cond { … }`
        // is a syntax error. Mirrors emit_assign's Var-branch wrap;
        // factored into the value-side emit so writer-suffix dispatch
        // (Send `title=`) and direct Var/Ivar assign produce the same
        // shape. IIFE return type comes from the value's analyzer-Ty
        // when it resolves to a concrete Go primitive (`string` for
        // type-assert-then-default patterns where both branches are
        // strings); falls back to `interface{}` when the analyzer
        // didn't pin a narrow type. The narrowed return type matters
        // because `instance.Title = func() interface{} {...}()` won't
        // fit a `string`-typed field; using the field's narrow type
        // lets the assignment typecheck.
        let value = &args[0];
        let v = if matches!(&*value.node, ExprNode::If { .. } | ExprNode::Case { .. }) {
            let ret_ty = super::ty::go_ty_stub(value.ty.as_ref());
            let body = emit_return_body(ctx, value);
            let indented = body
                .lines()
                .map(|l| format!("\t{l}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("func() {ret_ty} {{\n{indented}\n}}()")
        } else {
            args_s[0].clone()
        };
        return format!("{recv_s}.{field_go} = {v}");
    }

    // Ruby `recv[k] = v` is sugar for `recv.[]=(k, v)` in the IR.
    // For map/array receivers (Hash/Array/Untyped — the Ty-default
    // shape go2 emits as map/slice/interface{}), emit Go index-
    // assign. For Class-typed receivers (`self[:updated_at] = now`
    // inside an AR::Base method body, `@flash[:notice] = notice`
    // in an AC::Base method body), emit a method call to the
    // hand-defined `op_set` instead — Go structs aren't indexable,
    // so the index syntax would fail `go vet`. Union<Class, Nil>
    // peeled via union_non_nil_core so `Flash?` ivars still route.
    if method == "[]=" && args.len() == 2 && recv.is_some() {
        let recv_e = recv.unwrap();
        let recv_s = emit_expr(ctx, recv_e);
        // Q1 — `self[k] = v` inside an AR-class instance method
        // routes through the Modeler back-pointer so subclass OpSet
        // overrides fire. `self.OpSet(...)` would bind statically to
        // *ActiveRecordBase.OpSet (the panic stub) when called from
        // a Base method.
        if ctx.is_ar_class
            && !ctx.in_class_method
            && matches!(&*recv_e.node, ExprNode::SelfRef)
        {
            return format!("self.Self.OpSet({}, {})", args_s[0], args_s[1]);
        }
        if recv_ty_is_class(recv_e.ty.as_ref()) {
            return format!("{recv_s}.OpSet({}, {})", args_s[0], args_s[1]);
        }
        return format!("{recv_s}[{}] = {}", args_s[0], args_s[1]);
    }

    if method == "[]" && recv.is_some() {
        let recv_e = recv.unwrap();
        // Q1 — `self[k]` inside an AR-class instance method routes
        // through the Modeler back-pointer (mirrors `[]=` above).
        if ctx.is_ar_class
            && !ctx.in_class_method
            && matches!(&*recv_e.node, ExprNode::SelfRef)
        {
            return format!("self.Self.OpGet({})", args_s.join(", "));
        }
        if recv_ty_is_class(recv_e.ty.as_ref()) {
            let recv_s = emit_expr(ctx, recv_e);
            return format!("{recv_s}.OpGet({})", args_s.join(", "));
        }
        let recv_s = emit_expr(ctx, recv_e);
        // Ruby negative index (`recv[-1]`, `recv[-2]`, …) — Go has no
        // negative indexing on slices or strings; rewrite to
        // `recv[len(recv)-N]`. Gated on a literal `Int { value: < 0 }`
        // arg so non-literal negatives (`recv[i]` where i happens to
        // be negative at runtime) still emit the bare form and panic
        // at index time — matching the Go convention. recv_s is
        // emitted twice; safe for the Var/SelfRef/Const receivers the
        // runtime/ruby/ surface uses; side-effecting receivers would
        // re-evaluate (no current call site hits that).
        if args.len() == 1 {
            if let ExprNode::Lit { value: Literal::Int { value } } = &*args[0].node {
                if *value < 0 {
                    let offset = -*value;
                    return format!("{recv_s}[len({recv_s})-{offset}]");
                }
            }
        }
        // `str[start..end]` / `str[start..]` — Ruby Range becomes Go
        // slice syntax `str[start:end]`. Inclusive ranges add `+1` to
        // the end bound; exclusive ranges pass it through; open-ended
        // (no begin / no end) maps to Go's empty-side slice form.
        if args.len() == 1 {
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                let begin_s = begin
                    .as_ref()
                    .map(|e| emit_expr(ctx, e))
                    .unwrap_or_default();
                let end_s = match (end.as_ref(), *exclusive) {
                    (Some(e), true) => emit_expr(ctx, e),
                    (Some(e), false) => format!("{}+1", emit_expr(ctx, e)),
                    (None, _) => String::new(),
                };
                return format!("{recv_s}[{begin_s}:{end_s}]");
            }
        }
        // `str[start, length]` — Ruby's two-arg substring form. Map
        // to Go's `recv[start : start+length]`. Note: `start` is
        // emitted twice; safe for simple values (literal, var) but
        // re-evaluates side effects. Lowered runtime bodies don't
        // hit this pattern with side-effecting starts today; if
        // they do later, introduce a temp binding (needs statement
        // context, deferred).
        if args.len() == 2 {
            let start = &args_s[0];
            let length = &args_s[1];
            return format!("{recv_s}[{start}:{start}+{length}]");
        }
        return format!("{recv_s}[{}]", args_s.join(", "));
    }

    // Binary operators: Ruby parses `a == b`, `a + b`, etc. as
    // `Send { recv: a, method: "==", args: [b] }`. Emit them infix.
    if let (Some(r), Some(op)) = (recv, binary_op(method)) {
        if args.len() == 1 {
            return format!("{} {} {}", emit_expr(ctx, r), op, args_s[0]);
        }
    }

    // Ruby `.empty?` predicate on String/Array/Hash → Go
    // `len(recv) == 0`. Same scope as `length` — collection-shaped.
    if method == "empty?" && args.is_empty() {
        if let Some(r) = recv {
            return format!("len({}) == 0", emit_expr(ctx, r));
        }
    }

    // Ruby `arr << x` (Array push) parses as a Send `<<` with one
    // arg. Go: `arr = append(arr, x)`. Statement, not expression —
    // the lowered bodies use this at statement position. For Hash
    // shovel (`hash << pair`) the semantics differ; if a non-Array
    // recv hits this, the resulting Go `append(map, ...)` errors
    // at compile, surfacing the gap. `arr.push(x)` is the explicit-
    // method form of the same operation and gets the same rewrite.
    //
    // String-buffer shovel (`io << "..."` after `io = String.new`):
    // when the recv is a `string` (Str / Union<Str, Nil>), emit
    // string concat `io = io + X` instead of `append`. The view
    // bodies use this pattern extensively for HTML buffer
    // accumulation — `String.new` lowers to `""` (peepholed below)
    // and subsequent shovels stay as concat.
    if (method == "<<" || method == "push") && args.len() == 1 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let core_ty = r.ty.as_ref().and_then(union_non_nil_core);
            if matches!(core_ty, Some(Ty::Str)) {
                return format!("{recv_s} = {recv_s} + {}", args_s[0]);
            }
            return format!("{recv_s} = append({recv_s}, {})", args_s[0]);
        }
    }

    // Ruby `.freeze` / `.to_h` — both pass through the receiver
    // unchanged in Go (no immutability marker; `.to_h` is a no-op
    // on Ruby Hash and would convert NamedTuple → Hash under
    // Crystal, neither of which Go needs). Class-typed receivers
    // (`session.to_h` where Session defines `def to_h; @data; end`)
    // route through their explicit method instead — the peephole
    // shortcut would emit `session` which has the wrong type.
    if (method == "freeze" || method == "to_h") && args.is_empty() {
        if let Some(r) = recv {
            let core = r.ty.as_ref().and_then(union_non_nil_core);
            if !matches!(core, Some(Ty::Class { .. })) {
                return emit_expr(ctx, r);
            }
        }
    }

    // Ruby `recv.length` / `.size` → Go's `int64(len(recv))`. Works
    // on strings, slices, and maps — Go's `len()` is polymorphic
    // over those. The `int64(...)` wrap matches Ruby Integer's
    // mapping (int64 in go_ty_stub); without it, comparisons like
    // `i < keys.length` against an int64-typed `i` would fail. For
    // map receivers Go's `len()` returns int regardless of key/
    // value type, so no Ty plumbing needed at this site.
    if (method == "length" || method == "size") && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return format!("int64(len({recv_s}))");
        }
    }

    // Ruby `arr.count` (no args) — equivalent to `.size` on Arrays
    // and Hashes. View bodies hit this via `article.errors.count`.
    // Skip when recv is a Class-typed object (`Article.count` is the
    // AR class method, not a slice len) — handled separately by the
    // model adapter.
    if method == "count" && args.is_empty() {
        if let Some(r) = recv {
            let core_ty = r.ty.as_ref().and_then(union_non_nil_core);
            if matches!(core_ty, Some(Ty::Array { .. } | Ty::Hash { .. })) {
                let recv_s = emit_expr(ctx, r);
                return format!("int64(len({recv_s}))");
            }
        }
    }

    // Ruby `recv.empty?` → Go's `len(recv) == 0`. Same receiver
    // polymorphism as `.length`. For strings the Go `s == ""` form
    // is also valid and slightly cheaper, but `len(s) == 0` works
    // uniformly across the receiver Tys we see.
    if method == "empty?" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return format!("len({recv_s}) == 0");
        }
    }

    // Ruby `h.keys` → Go IIFE collecting all map keys. Go has no
    // builtin Hash#keys; the idiom is `for k := range m { ... }`
    // for direct iteration. When the Ruby code wants the materialized
    // slice (`keys = m.keys; while i < keys.length; k = keys[i]`),
    // we synthesize it. NOTE: Go map iteration order is undefined
    // per-run; Ruby Hash is insertion-ordered. For runtime-Ruby
    // surfaces like Session/Flash where order isn't load-bearing
    // this is fine; flag if app-level code depends on Ruby-style
    // ordering.
    if method == "keys" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let (k_ty, _v_ty) = hash_kv_go_tys(r.ty.as_ref());
            return format!(
                "func() []{k_ty} {{\n\
                 \t_ks := make([]{k_ty}, 0, len({recv_s}))\n\
                 \tfor _k := range {recv_s} {{\n\
                 \t\t_ks = append(_ks, _k)\n\
                 \t}}\n\
                 \treturn _ks\n\
                 }}()",
            );
        }
    }

    // Ruby `h.values` → Go IIFE collecting all map values. Symmetric
    // with `.keys`. Order caveat identical.
    if method == "values" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let (_k_ty, v_ty) = hash_kv_go_tys(r.ty.as_ref());
            return format!(
                "func() []{v_ty} {{\n\
                 \t_vs := make([]{v_ty}, 0, len({recv_s}))\n\
                 \tfor _, _v := range {recv_s} {{\n\
                 \t\t_vs = append(_vs, _v)\n\
                 \t}}\n\
                 \treturn _vs\n\
                 }}()",
            );
        }
    }

    // Ruby `h.key?(k)` / `h.has_key?(k)` / `h.include?(k)` → Go's
    // comma-ok membership form. IIFE wrap keeps it expression-shaped
    // (Ruby uses `.key?` as a condition; Go's `_, ok := m[k]; ok`
    // is statement-shaped at top level). The `include?` variant
    // overlaps with Array#include? (membership scan) — for non-Hash
    // receivers (Array, Range) this emit is wrong; the runtime/ruby
    // surface only uses the Hash form today, so the gap stays loud.
    if (method == "key?" || method == "has_key?" || method == "include?")
        && args.len() == 1
    {
        if let Some(r) = recv {
            // Only fire when recv looks like a Hash. Otherwise (Array
            // recv for `include?`) defer to the existing slices.Contains
            // path further down. Nested Unions (`Union[Hash, Nil]` for
            // nullable maps) flatten through `union_non_nil_core`.
            let core = r.ty.as_ref().and_then(union_non_nil_core);
            if matches!(core, Some(Ty::Hash { .. }))
                || (method != "include?" && r.ty.is_none())
            {
                let recv_s = emit_expr(ctx, r);
                let key_s = &args_s[0];
                return format!(
                    "func() bool {{ _, ok := {recv_s}[{key_s}]; return ok }}()",
                );
            }
        }
    }

    // Ruby `!x` parses two ways depending on the parser path:
    //   - `Send { recv: x, method: "!", args: [] }` (method-style)
    //   - `Send { recv: None, method: "!", args: [x] }` (op-style)
    // Both reach Go as the unary `!(...)`. Without the no-recv arm,
    // the op-style form falls through to `Bang(...)` via the
    // `go2_method_ident`-then-call dispatch (sees `!` as a method
    // name and pascalizes it to `Bang`).
    if method == "!" && args.len() == 1 && recv.is_none() {
        return format!("!({})", args_s[0]);
    }
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!({})", emit_expr(ctx, r));
        }
    }

    // Ruby `h.dup` → IIFE returning a shallow copy. Map shape:
    // allocate a fresh map of the same K/V Tys and range-copy. For
    // non-Hash receivers (Array#dup, String#dup) the emit would
    // need different shapes; runtime/ruby/ only uses Hash#dup
    // today.
    if method == "dup" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let (k_ty, v_ty) = hash_kv_go_tys(r.ty.as_ref());
            return format!(
                "func() map[{k_ty}]{v_ty} {{\n\
                 \t_out := map[{k_ty}]{v_ty}{{}}\n\
                 \tfor _k, _v := range {recv_s} {{ _out[_k] = _v }}\n\
                 \treturn _out\n\
                 }}()",
            );
        }
    }

    // Ruby `h.merge(other)` on a Hash receiver → IIFE that copies
    // both maps into a fresh `map[string]any`. Ruby Hash#merge is
    // heterogeneous (string-valued + symbol-keyed entries flow
    // together with any-valued + arbitrary-keyed entries), so the
    // result type widens to `map[string]any`. The IIFE coerces each
    // source value through `any` to bridge any narrower input
    // (`map[string]string` → values lifted to interface{}). Symbol
    // keys flow through Ruby Hash literals' Symbol→String rendering
    // already; if either side somehow carries non-string keys, the
    // existing `fmt.Sprintf("%v", k)` shape covers that, but the
    // narrow-key path is what real-blog hits.
    if method == "merge" && args.len() == 1 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let arg_s = &args_s[0];
            return format!(
                "func() map[string]any {{\n\
                 \t_out := map[string]any{{}}\n\
                 \tfor _k, _v := range {recv_s} {{ _out[_k] = _v }}\n\
                 \tfor _k, _v := range {arg_s} {{ _out[_k] = _v }}\n\
                 \treturn _out\n\
                 }}()",
            );
        }
    }

    // Ruby `h.delete(k)` → Go IIFE returning the deleted value (or
    // `nil` if absent) and then calling Go's `delete()` builtin.
    // Ruby's Hash#delete returns the value, so emit-only `delete(h,
    // k)` (void) breaks any return-position use. The IIFE shape
    // works in both statement and return positions; statement use
    // discards the value, matching Go's "discarded return is fine"
    // semantics. For non-Hash receivers (Array#delete-by-value)
    // this emit is wrong; the runtime/ruby/ surface only uses the
    // Hash form today.
    if method == "delete" && args.len() == 1 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let key_s = &args_s[0];
            let (_k_ty, v_ty) = hash_kv_go_tys(r.ty.as_ref());
            return format!(
                "func() {v_ty} {{\n\
                 \t_v, _ok := {recv_s}[{key_s}]\n\
                 \tdelete({recv_s}, {key_s})\n\
                 \tif _ok {{ return _v }}\n\
                 \tvar _zero {v_ty}\n\
                 \treturn _zero\n\
                 }}()",
            );
        }
    }

    // Ruby `h.fetch(k, default)` → Go `cmp.Or(h[k], default)`.
    // Subtle semantic gap: Ruby fetch returns default ONLY if the
    // key is missing; cmp.Or returns default when h[k] is the zero
    // value (which is "" / 0 / nil for missing keys but also for
    // explicitly-stored zero values). Acceptable for the
    // runtime/ruby/ surface today; revisit if call sites
    // distinguish missing vs zero.
    //
    // When the default is Ruby `nil` and the receiver's value type
    // is a non-nilable Go scalar (string/int64/float64/bool),
    // substitute the Go zero value — `cmp.Or` is generic over
    // `comparable`, so both args must share a concrete type. Ruby
    // `nil` is the canonical empty signal; mapping it to "" / 0 /
    // false preserves the Ruby semantics (`hash[k]` reads "" for a
    // missing string-valued key in Go anyway, so the fallback is
    // redundant in the missing-key case but still right in the
    // explicit-default case).
    if method == "fetch" && args.len() == 2 {
        if let Some(r) = recv {
            let (_k_ty, v_ty) = hash_kv_go_tys(r.ty.as_ref());
            let default_s = if matches!(*args[1].node, ExprNode::Lit { value: Literal::Nil }) {
                match v_ty.as_str() {
                    "string" => "\"\"".to_string(),
                    "int64" => "int64(0)".to_string(),
                    "float64" => "0.0".to_string(),
                    "bool" => "false".to_string(),
                    _ => args_s[1].clone(),
                }
            } else {
                args_s[1].clone()
            };
            // `cmp.Or[T]` requires both args share the receiver's
            // value type T. When T is the `any`-alias `RoundhouseParamValue`
            // (the recursive param tree) but the default literal renders
            // as a concrete map/array/scalar, Go's generic inference
            // settles on the concrete shape and rejects `params[k]`
            // (typed `any`). Lift the default into T explicitly — for
            // alias-to-`any` types Go accepts any RHS via the conversion
            // syntax, so `RoundhouseParamValue(map[string]…{})` and
            // `any(…)` both unify the args under T=any.
            let default_s = if default_needs_v_ty_cast(&v_ty, &default_s) {
                format!("{v_ty}({default_s})")
            } else {
                default_s
            };
            return format!(
                "cmp.Or({}[{}], {})",
                emit_expr(ctx, r),
                args_s[0],
                default_s
            );
        }
    }

    // Ruby `s.tr(from, to)` (1-char from/to) → Go's
    // `strings.ReplaceAll(s, from, to)`. Multi-char tr (`tr("ab",
    // "12")` mapping a→1, b→2) needs per-char translation —
    // approximate with ReplaceAll for now; gap surfaces only on
    // multi-char tr which the runtime doesn't use.
    if method == "tr" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "strings.ReplaceAll({}, {}, {})",
                emit_expr(ctx, r),
                args_s[0],
                args_s[1]
            );
        }
    }

    // Ruby `arr.join(sep)` → Go `strings.Join(arr, sep)` when the
    // receiver renders to `[]string`. Bare `arr.join` (no arg)
    // matches Ruby's default of an empty separator. For wider
    // element types (`[]interface{}`, `[]any`, etc. — usually a
    // `parts = []` Var whose elem Ty wasn't back-propagated from
    // later `<<`/`push` sites) wrap with a range-to-string-slice
    // IIFE so each element gets the `%v` stringify before
    // `strings.Join` runs.
    if method == "join" && args.len() <= 1 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let recv_go_ty = super::ty::go_ty_stub(r.ty.as_ref());
            let sep = if args.is_empty() {
                "\"\"".to_string()
            } else {
                args_s[0].clone()
            };
            if recv_go_ty == "[]string" {
                return format!("strings.Join({recv_s}, {sep})");
            }
            return format!(
                "strings.Join(func() []string {{\n\
                 \t_out := make([]string, 0, len({recv_s}))\n\
                 \tfor _, _v := range {recv_s} {{ _out = append(_out, fmt.Sprintf(\"%v\", _v)) }}\n\
                 \treturn _out\n\
                 }}(), {sep})",
            );
        }
    }

    // Ruby `recv.gsub(pattern_regex, hash_or_string)` →
    // `pattern.ReplaceAll{String,StringFunc}(recv, ...)` against
    // Go's regexp package. Hash replacement uses
    // `ReplaceAllStringFunc` with a closure that looks up each
    // match in the hash; string replacement uses `ReplaceAllString`.
    // Heuristic: if arg[1]'s emit references a map-shape identifier
    // we can't tell from here — pick by `Expr` shape (Const →
    // hash lookup, StringInterp / Lit::Str → string replacement).
    // For json_builder's `s.gsub(ESCAPE_PATTERN, ESCAPES)` both
    // args are Const, so we land on the hash-lookup form.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            // The framework HTML escaper — `s.gsub(HTML_ESCAPE_PATTERN,
            // HTML_ESCAPES)` — lowers here. Go's
            // `regexp.ReplaceAllStringFunc` for a five-character class
            // runs the full regex engine plus a per-match closure and map
            // lookup, allocating on every call. The published bench had
            // Go's HTML endpoints anomalously slow (a compiled target below
            // TypeScript on the DB-less /articles/new render), and a
            // cross-target sweep showed Go alone routing this escaper
            // through the regex engine. `html.EscapeString` is
            // the stdlib single-pass `strings.Replacer` (no regex, built
            // once). It emits `&#34;` for `"` where the framework map uses
            // `&quot;`, but the two are DOM-equivalent — html5ever decodes
            // both to `"`, and that DOM equivalence (not byte identity) is
            // exactly what `compare` asserts. Same five characters escaped,
            // same rendered DOM, native speed. Keyed on the `HTML_ESCAPES`
            // constant so the json_builder escaper (`ESCAPES`, a different
            // character set) keeps the generic form below.
            if let ExprNode::Const { path } = &*args[1].node {
                if path.last().map(|s| s.as_str()) == Some("HTML_ESCAPES") {
                    return format!("html.EscapeString({recv_s})");
                }
            }
            let pattern = emit_expr(ctx, &args[0]);
            let replacement = emit_expr(ctx, &args[1]);
            let is_string_repl = matches!(
                &*args[1].node,
                ExprNode::Lit { value: Literal::Str { .. } }
                    | ExprNode::StringInterp { .. }
            );
            if is_string_repl {
                return format!("{pattern}.ReplaceAllString({recv_s}, {replacement})");
            }
            return format!(
                "{pattern}.ReplaceAllStringFunc({recv_s}, func(m string) string {{ \
                 return {replacement}[m] }})"
            );
        }
    }

    // Ruby `recv.is_a?(SingletonClass)` for the three singleton
    // boolean-/nil-classes maps to direct equality, which is a plain
    // bool expression that fits anywhere `is_a?` appears (not just
    // at `if`-cond position). The mappable-class cases (Integer,
    // Float, String, ...) require Go's `_, ok := v.(T)` form, which
    // is statement-level and gets handled by the per-If/per-return
    // walker (`emit_expr::If` + `emit_return_at::If`).
    if method == "is_a?" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*args[0].node {
                if let Some(class) = path.last() {
                    let recv_s = emit_expr(ctx, r);
                    match class.as_str() {
                        "TrueClass" => return format!("{recv_s} == true"),
                        "FalseClass" => return format!("{recv_s} == false"),
                        "NilClass" => return format!("{recv_s} == nil"),
                        _ => {}
                    }
                }
            }
        }
    }

    // Ruby `.nil?` predicate → Go nil comparison. The receiver's
    // Ty (when known) drives the result:
    //
    //   non-nilable primitives (Str, Sym, Int, Float, Bool) — Ruby
    //     `nil?` is statically false; emit `false`. Avoids invalid
    //     Go like `string == nil`.
    //   Ty::Nil — statically true.
    //   anything else (Untyped, Union, Class, unknown) — emit
    //     `recv == nil`, which works against interface{}, pointer,
    //     map, slice, and channel receivers.
    //
    // Without analyzer-filled `Expr.ty` the receiver appears
    // typeless and falls to `== nil`, which is the safe default for
    // the `interface{}` shape go2 emits for unknown types.
    if method == "nil?" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return match r.ty.as_ref() {
                Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool) => {
                    "false".to_string()
                }
                Some(Ty::Nil) => "true".to_string(),
                Some(ty @ Ty::Union { .. }) => {
                    // Empty-as-nil for `Union[Str/Sym, Nil]` matches
                    // the go_ty_stub convention. `Union[Hash/Array/
                    // Class, Nil]` still maps to Go reference types
                    // where `== nil` is valid (slice/map/pointer);
                    // the catchall preserves that path. Nested
                    // Unions (`Union[Union[Str, Nil], Nil]`, seen
                    // when the analyzer wraps a nullable field's
                    // Ty in another nullable layer) flatten through
                    // `union_non_nil_core`.
                    match union_non_nil_core(ty) {
                        Some(Ty::Str | Ty::Sym) => format!("{recv_s} == \"\""),
                        _ => format!("{recv_s} == nil"),
                    }
                }
                _ => format!("{recv_s} == nil"),
            };
        }
    }

    // Ruby `.to_s` → Go string conversion. String-typed receiver is
    // a no-op; numeric receivers use the matching Sprintf verb;
    // everything else (including untyped `interface{}`) falls back
    // to `%v` — which delegates to `fmt.Stringer` if implemented
    // and is a reasonable default otherwise. Without analyzer-filled
    // `Expr.ty` the receiver appears typeless and we land on `%v`.
    if method == "to_s" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return match r.ty.as_ref() {
                Some(Ty::Str | Ty::Sym) => recv_s,
                Some(Ty::Int) => format!("fmt.Sprintf(\"%d\", {recv_s})"),
                Some(Ty::Float) => format!("fmt.Sprintf(\"%g\", {recv_s})"),
                _ => format!("fmt.Sprintf(\"%v\", {recv_s})"),
            };
        }
    }

    // `self.class.X(args)` — Ruby idiom for "dispatch X on the class
    // of self" (`self.class.schema_columns` in
    // `ActiveRecord::Base#fill_timestamps`). The chained Send lowers
    // to `Send { recv: Send { recv: SelfRef, method: "class" },
    // method: X }`. Go has no inheritance / Self resolution; rewrite
    // to the enclosing-class bare-fn call `<ClassName>_X(args)`,
    // matching the rust2 `Self::X(args)` strategy. Subclass overrides
    // aren't routed by this rewrite — they'll need interface dispatch
    // or a per-instance vtable later; for the runtime walker today
    // it's enough to emit a syntactically valid call into the
    // enclosing-class slot (Base's body panics with NotImplementedError,
    // which surfaces the override gap as a runtime error not a
    // compile error). Only fires when the inner recv is SelfRef and
    // we have a known enclosing class name; other `.class` chains
    // (`record.class.foo`) fall through and surface as proper Go
    // build errors upstream.
    if let (Some(r), Some(class_name)) = (recv, ctx.class_name.as_deref()) {
        if let ExprNode::Send {
            recv: Some(inner_recv),
            method: inner_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if inner_method.as_str() == "class"
                && inner_args.is_empty()
                && matches!(&*inner_recv.node, ExprNode::SelfRef)
            {
                // `self.class.name` resolves to a string literal of
                // the enclosing class name (same shape as the bare-
                // `name`-in-class-method peephole above).
                if method == "name" && args.is_empty() {
                    return format!("{class_name:?}");
                }
                // Q1 — inside an AR-chain class's instance method,
                // route `self.class.X` through the Modeler interface
                // back-pointer (`self.Self.X()`) so subclass overrides
                // fire at runtime. The Self field is on the embedded
                // *ActiveRecordBase; constructor wiring of the outer
                // subclass made it point at the outer instance, so
                // the interface dispatch resolves to the outer's
                // method. Limited to the Modeler interface members
                // (currently schema_columns, op_get, op_set); other
                // class methods stay on the static `<Class>_X()` form
                // since they're not in the Modeler surface.
                if ctx.is_ar_class && !ctx.in_class_method && is_modeler_member(method) {
                    let pascal = go_method_name(&super::library::sanitize_method_name(method));
                    return format!("self.Self.{pascal}({})", args_s.join(", "));
                }
                let m = super::library::sanitize_method_name(method);
                return format!("{class_name}_{m}({})", args_s.join(", "));
            }
        }
    }

    // Q1 — bare `self.X(args)` inside an AR-chain class's instance
    // method, where X is a Modeler interface member. `(*ActiveRecordBase).Save`
    // body emits `self.AdapterInsert()`, `self.Validate()`,
    // `self.AfterCreateCommit()` etc.; without rewriting, Go binds
    // these statically to Base's stub (returns 0 / empty body) because
    // self's type IS *ActiveRecordBase inside Base's method. Route
    // through the Self back-pointer so subclass overrides fire — same
    // mechanism as `self.class.X` above and `self[k]=v` / `self[k]`
    // earlier in this function. Modeler membership gates which methods
    // qualify; non-polymorphic methods (Save, NewRecord, FillTimestamps,
    // OpResolveStatus) stay on the static binding because they're not
    // overridden by subclasses.
    if let Some(r) = recv {
        if ctx.is_ar_class
            && !ctx.in_class_method
            && matches!(&*r.node, ExprNode::SelfRef)
            && is_modeler_member(method)
        {
            let pascal = go_method_name(&super::library::sanitize_method_name(method));
            return format!("self.Self.{pascal}({})", args_s.join(", "));
        }
    }

    // SelfRef receiver in a class method context — rewrite to the
    // bare-fn call `ClassName_method(args)` because class methods
    // emit as bare functions, not as methods on a struct. Use the
    // bare-fn name shape (lowercase + `?`/`!`/`=` sanitized, no
    // pascalize) to match the function-definition shape in
    // `library::emit_method`.
    if let (Some(r), Some(class_name)) = (recv, ctx.class_name.as_deref()) {
        if ctx.in_class_method && matches!(&*r.node, ExprNode::SelfRef) {
            let m = super::library::sanitize_method_name(method);
            return format!("{class_name}_{m}({})", args_s.join(", "));
        }
    }

    // `String.new` → `""`; `String.new(s)` → `s`. Ruby's
    // `String.new` produces an empty string buffer; view bodies use
    // it for HTML accumulation (`io = String.new; io << "..."`).
    // Go has no string-buffer type — the `<<` shovel peephole above
    // routes Str-typed recv to `io = io + X` concat, so the buffer
    // collapses to a plain `string`. Intercept before the general
    // `Const.new` arm so we don't generate an undefined `NewString`.
    if method == "new" {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "String" {
                    return match args_s.len() {
                        0 => "\"\"".to_string(),
                        _ => args_s[0].clone(),
                    };
                }
            }
        }
    }

    // `<Const>.new(args)` → `New<Const>(args)`. The constructor is
    // synthesized by `library::emit_constructor` for any class with
    // an `initialize` method; this rewrite makes the Ruby `.new`
    // call site target it.
    //
    // Exception: `Views::<X>.new(args)` is a view module function
    // call (the `new` action template), not a constructor — the
    // view_to_library lowerer emits `Views::Articles`, `Views::
    // Comments`, etc. as modules of bare functions, not classes
    // with `initialize`. Falling through here lets the generic
    // `Const(X).method(args)` → `X_method(args)` dispatch below
    // produce `ViewsArticles_new(args)`, which matches the stub
    // signature emitted by `views_articles_stubs.go`.
    if method == "new" {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                let is_view_module = path
                    .first()
                    .map(|s| s.as_str() == "Views")
                    .unwrap_or(false);
                if !is_view_module {
                    let class_name = path
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    return format!("New{class_name}({})", args_s.join(", "));
                }
            }
        }
        // Bare `new(args)` inside a class method (`def self.create;
        // new(attrs); end`) — Ruby resolves the implicit receiver to
        // the enclosing class. Route to the synthesized constructor
        // for that class so emit produces `New<ClassName>(args)`,
        // not an undefined-identifier `New(args)`.
        if recv.is_none() && ctx.in_class_method {
            if let Some(class) = ctx.class_name.as_deref() {
                return format!("New{class}({})", args_s.join(", "));
            }
        }
    }

    // Stdlib-module call rewrites where the Ruby receiver is a
    // `Const` referencing a module name with a Go equivalent.
    // Base64.strict_encode64(s) → base64.StdEncoding.EncodeToString
    // ([]byte(s)). JSON.generate(x) → IIFE around json.Marshal that
    // discards the error (mirrors Ruby's "raises on bad encode" by
    // letting Go's encoder produce an empty string for the unhappy
    // path; runtime/ruby/ inputs are well-formed today). Both require
    // imports — `needed_imports` probes for `base64.`/`json.` to pull
    // them in.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            let module = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("");
            if module == "Base64" && method == "strict_encode64" && args.len() == 1 {
                return format!(
                    "base64.StdEncoding.EncodeToString([]byte({}))",
                    args_s[0]
                );
            }
            if module == "JSON" && method == "generate" && args.len() == 1 {
                return format!(
                    "func() string {{ _b, _ := json.Marshal({}); return string(_b) }}()",
                    args_s[0]
                );
            }
        }
    }

    // Receiver-Ty-aware dispatch for methods whose name is ambiguous
    // between String and Array receivers (`include?` is both
    // `strings.Contains` and `slices.Contains`). When `recv.ty` carries
    // an `Array { .. }`, route to the slice form and skip the unguarded
    // string-method fallback below. Sym/Str arrays both collapse to
    // `[]string` via `go_ty_stub`, so the slice-elem comparison works
    // unchanged.
    if method == "include?" && args.len() == 1 {
        if let Some(r) = recv {
            if matches!(r.ty.as_ref(), Some(Ty::Array { .. })) {
                let recv_s = emit_expr(ctx, r);
                return format!("slices.Contains({recv_s}, {})", args_s[0]);
            }
        }
    }

    // Ruby→Go method-name mapping for string operations that have no
    // 1:1 in Go's stdlib (`strip` is `strings.TrimSpace(…)`, not
    // `.Strip()`). Applied whenever the method name matches the
    // known str-specific set — we don't gate on receiver Ty because
    // many lowered Var receivers come through with `Ty::Untyped`
    // (signature said `untyped`) or with no analyzer-set Ty at all.
    // A wrong hit (e.g. an Array's `include?` rendering as
    // `strings.Contains`) emits invalid Go and surfaces the gap;
    // silently emitting `.Include(...)` would produce an undefined-
    // method error that's harder to debug. Array `include?` is
    // intercepted above when the receiver Ty is known.
    if let Some(r) = recv {
        let recv_s = emit_expr(ctx, r);
        if args.is_empty() {
            if let Some(wrapped) = map_go_str_method(method, &recv_s) {
                return wrapped;
            }
        }
        if args.len() == 1 {
            if let Some(wrapped) = map_go_str_method_1arg(method, &recv_s, &args_s[0]) {
                return wrapped;
            }
        }
    }

    let go_m = go2_method_ident(method);
    match recv {
        None => {
            // Bareword identifier in an expression context — Ruby's
            // parser can't tell `article` (a local/param ref) from
            // `article()` (an implicit-self method call), so both
            // ingest as `Send { recv: None, method: "article" }`.
            // When the bareword name matches a parameter or local
            // declared in scope, treat it as a Var read — emit the
            // sanitized lowercase name instead of the pascalized
            // method ident. Without this, view bodies that reference
            // their `article` / `comment` params produce
            // `Article.Persisted()` instead of `article.Persisted`.
            if args_s.is_empty() && ctx.declared.borrow().contains(method) {
                return super::library::sanitize(method);
            }
            if args_s.is_empty() {
                // Bare reader position (no parens) → field-read form,
                // suffix stripped (`persisted?` → `Persisted`).
                go_field_ident(method)
            } else {
                format!("{}({})", go_m, args_s.join(", "))
            }
        }
        Some(r) => {
            // `Const(X).method(args)` → `X_method(args)`. Go has no
            // method-dispatch syntax on a type (vs. an instance), so
            // class methods live as bare functions under the
            // `<Class>_<method>` convention that matches what
            // `library::emit_method` produces for `def self.X`. The
            // sanitize-and-snake form preserves Ruby `?`/`!`/`=`
            // suffixes the same way method-decl emit does. The
            // `.new` special case (above) intercepts constructor
            // dispatch before this fires.
            if let ExprNode::Const { path } = &*r.node {
                let class_name = path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                let method_sanitized = super::library::sanitize_method_name(method);
                return format!(
                    "{class_name}_{method_sanitized}({})",
                    args_s.join(", "),
                );
            }
            let recv_s = emit_expr(ctx, r);
            // Struct field access vs method call: 0-arg Sends on a
            // non-Class receiver whose method isn't a known AR/stdlib
            // call render without parens (`p.Title`, not `p.Title()`).
            //
            // Exception: implicit-self calls to a method DEFINED on
            // the enclosing class (`before_validation` inside
            // `save`'s body lowers to `Send { recv: SelfRef,
            // method: before_validation }`) must emit with parens so
            // the dispatch fires. Detection routes via
            // `ctx.self_methods` (populated by library::emit_method);
            // attr_reader/writer-backed struct fields are NOT in
            // that set, so `self.id` (an attr_accessor) still emits
            // bare as `self.ID`.
            if args_s.is_empty() && !is_known_go_method(method) {
                let is_self_call = matches!(&*r.node, ExprNode::SelfRef);
                let in_self_methods = is_self_call
                    && ctx
                        .self_methods
                        .as_ref()
                        .map(|set| set.contains(method))
                        .unwrap_or(false);
                if in_self_methods {
                    return format!("{recv_s}.{go_m}()");
                }
                // Force call-form for known non-field methods on
                // typed receivers. The 0-arg-Send-as-field-read
                // heuristic above defaults to bare property access
                // (the right shape for AR struct fields like
                // `record.id` / `record.title`), but real methods
                // such as `dom_prefix` (lowerer-synthesized per-
                // model + panic-overridden on Base) MUST emit with
                // parens — bare `record.DomPrefix` is a func-value
                // reference, which `go vet` flags.
                //
                // TODO: replace with the eventual IR-level
                // `Send.parenthesized` flag (TS already consumes it
                // — see src/emit/typescript/expr.rs:2555). The
                // syntactic `record.dom_prefix()` parens the author
                // wrote should survive ingest into the IR so
                // emitters that distinguish method-call vs field-
                // read can honor it without per-target heuristic
                // lists. Complementary piece: per-class
                // AccessorKind registry threaded through ctx so
                // call sites that omit parens still resolve
                // definitionally.
                if is_known_class_method(method) {
                    return format!("{recv_s}.{go_m}()");
                }
                // Bare field read (no parens) → suffix-stripped field
                // name so `record.persisted?` → `record.Persisted`.
                return format!("{recv_s}.{}", go_field_ident(method));
            }
            format!("{}.{}({})", recv_s, go_m, args_s.join(", "))
        }
    }
}

/// Emit an `ExprNode::Cast` — the IR's explicit "treat the inner
/// value as this Ty at the use site" marker. The
/// `lower::ty_coerce_insertion` lowerer inserts these at call-site
/// arg positions where the callee's declared param Ty widens the arg.
///
/// Two families consumed here:
///
/// - **Hash widening**: target `Hash<_, untyped>`
///   (`map[string]interface{}`) with a narrower source map. Emit IIFE
///   that ranges over the source and copies into a new
///   `map[string]interface{}`. Go doesn't auto-widen map element
///   types; the per-entry copy is the only generic shape that
///   survives `go vet`.
///
/// - **Value → primitive narrowing**: target `Str`/`Sym` with a
///   source whose Ty contains `Untyped` (boxed value flowing into a
///   typed slot). Emit `fmt.Sprintf("%v", inner)` — robust against
///   any source type, identity for strings under the `%v` verb's
///   String-call semantics. Int/Float/Bool narrowing intentionally
///   deferred: Go's `interface{}` → numeric type-assert path is
///   per-arity (int / int32 / int64 / float64) and needs receiver-
///   Ty knowledge that's better handled when a concrete site demands it.
///
/// Other Cast targets fall back to identity — each subsequent family
/// adds an arm here.
fn emit_cast(ctx: &EmitCtx, value: &Expr, target_ty: &Ty) -> String {
    let inner = emit_expr(ctx, value);
    if let Ty::Hash { value: tv, .. } = target_ty {
        if matches!(tv.as_ref(), Ty::Untyped) {
            let tgt = super::ty::go_ty_stub(Some(target_ty));
            let src = super::ty::go_ty_stub(value.ty.as_ref());
            if src != tgt {
                return format!(
                    "func() {tgt} {{ _src := {inner}; _out := make({tgt}, len(_src)); for k, v := range _src {{ _out[k] = v }}; return _out }}()"
                );
            }
        }
    }
    // Source/target Go-type identity check shared by the primitive
    // arms below. The Cast IIFE is wasteful (and invalid) when the
    // inner already has the target Go type — `row.ID` typed `int64`
    // flowing into an `int64` slot doesn't need a type assertion;
    // `(row.ID).(int64)` is a vet error since `row.ID` isn't an
    // interface. Identity-emit when source Ty's go_ty_stub already
    // matches the target's. Mirrors the Hash-widening arm above.
    let tgt = super::ty::go_ty_stub(Some(target_ty));
    let src = super::ty::go_ty_stub(value.ty.as_ref());
    let already_typed = src == tgt;

    // Value → primitive narrowing (Str/Sym). The lowerer's
    // `needs_value_to_primitive` gate ensures we only see this when
    // the source Ty actually contains Untyped — no widening-only or
    // already-typed args reach here.
    if matches!(target_ty, Ty::Str | Ty::Sym) {
        if already_typed {
            return inner;
        }
        return format!("fmt.Sprintf(\"%v\", {inner})");
    }
    // Value → primitive narrowing (Int/Float/Bool). The lowerer wraps
    // interface{}-yielding expressions in Cast(_, primitive) at Send-
    // arg positions. Go can't auto-convert interface{} to a typed
    // scalar — emit a type-asserting IIFE that returns the Go zero
    // value when the assertion fails, mirroring Ruby's `nil`/missing-
    // key fallback to numeric zero. `v, _ := <inner>.(T)` keeps the
    // ok flag discarded since the caller's site already has its own
    // default-handling (via the BoolOp::Or → cmp.Or peephole).
    let primitive_go_ty = match target_ty {
        Ty::Int => Some("int64"),
        Ty::Float => Some("float64"),
        Ty::Bool => Some("bool"),
        _ => None,
    };
    if let Some(go_ty) = primitive_go_ty {
        if already_typed {
            return inner;
        }
        // For Int/Float targets prefer Sprintf+ParseInt/ParseFloat
        // over raw type assertion. Type assertion `(x).(int64)` only
        // succeeds when the runtime concrete type IS exactly int64;
        // params-derived values are strings (`params[:id]` lands as
        // `string` in the underlying map), so `(value).(int64)` would
        // panic at runtime even when assertion compiles. Sprintf+Parse
        // handles both shapes uniformly — int64 round-trips through
        // "%v" formatting, string parses directly, nil/missing falls
        // back to Go zero. Boolean stays on type assertion since
        // Ruby's `to_b` idiom doesn't survive the cross-target path.
        match target_ty {
            Ty::Int => {
                return format!(
                    "func() int64 {{ n, _ := strconv.ParseInt(strings.TrimSpace(fmt.Sprintf(\"%v\", {inner})), 10, 64); return n }}()"
                );
            }
            Ty::Float => {
                return format!(
                    "func() float64 {{ n, _ := strconv.ParseFloat(strings.TrimSpace(fmt.Sprintf(\"%v\", {inner})), 64); return n }}()"
                );
            }
            _ => {}
        }
        return format!(
            "func() {go_ty} {{ _v, _ := ({inner}).({go_ty}); return _v }}()",
        );
    }
    inner
}

/// `SelfRef` in value position (not as a Send receiver). Class
/// methods don't have a `self` binding in Go, so emit the class
/// name itself as the most useful identifier-shape stand-in. Code
/// downstream of this fallback might still produce nonsense, but at
/// least the file parses and the gap is locally visible.
fn self_ref_expr(ctx: &EmitCtx) -> String {
    if ctx.in_class_method {
        match ctx.class_name.as_deref() {
            Some(n) => n.to_string(),
            None => "/* TODO: SelfRef without class context */".to_string(),
        }
    } else {
        // Instance method — the emitted Go has `(self *Class)` and
        // `self` is the receiver param name.
        "self".to_string()
    }
}

/// Ruby method names with `?` / `!` suffixes don't translate to Go
/// identifiers. Rewrite to `_pred` / `_bang` form before passing
/// through the standard pascalize path: `nil?` → `nil_pred` → `NilPred`,
/// `save!` → `save_bang` → `SaveBang`. Semantic translation
/// (`nil?` → `== nil`, `is_a?(C)` → type assertion) is a separate
/// widening; this only handles the identifier-shape side so emit
/// produces parseable Go.
/// Field-read form of a reader name: strips the `?` predicate suffix
/// (Go struct fields never carry it) so a 0-arg field read of
/// `persisted?` lands on the `Persisted` field, not the affixed
/// `PersistedPred` method ident. Bare (no-paren) reader positions use
/// this; method-CALL positions use `go2_method_ident`, which affixes
/// `?` → `_pred` to disambiguate from a same-named reader/field.
fn go_field_ident(ruby_name: &str) -> String {
    let stripped = ruby_name.strip_suffix('?').unwrap_or(ruby_name);
    let normalized = stripped.replace('!', "_bang");
    go_method_name(&normalized)
}

fn go2_method_ident(ruby_name: &str) -> String {
    // Mirror rust2's `sanitize_ident` (src/emit/rust2/expr/util.rs):
    // `?` predicate suffix maps to `_pred`, `!` maps to `_bang`. The `?`
    // affix (vs a bare strip) keeps a column predicate distinct from its
    // same-named reader (`deleted_at?` → `DeletedAtPred` vs `deleted_at`
    // → `DeletedAt`), applied unconditionally so def and call sites agree
    // and hand-written adapter methods conform to the same convention.
    let with_pred = match ruby_name.strip_suffix('?') {
        Some(base) => format!("{base}_pred"),
        None => ruby_name.to_string(),
    };
    let normalized = with_pred.replace('!', "_bang");
    go_method_name(&normalized)
}

fn is_nil_lit(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// True iff `recv` is exactly the chain `Time.now.utc` — paired with
/// an outer `.iso8601()` Send to produce
/// `time.Now().UTC().Format(time.RFC3339)`. Walks two nested Sends
/// (`.utc` outer, `.now` inner) then verifies the receiver is the
/// top-level `Time` constant.
fn is_time_now_utc_chain(recv: Option<&Expr>) -> bool {
    let Some(utc_expr) = recv else { return false };
    let ExprNode::Send {
        recv: now_recv,
        method: utc_method,
        args: utc_args,
        ..
    } = &*utc_expr.node
    else {
        return false;
    };
    if utc_method.as_str() != "utc" || !utc_args.is_empty() {
        return false;
    }
    let Some(now_expr) = now_recv else { return false };
    let ExprNode::Send {
        recv: time_recv,
        method: now_method,
        args: now_args,
        ..
    } = &*now_expr.node
    else {
        return false;
    };
    if now_method.as_str() != "now" || !now_args.is_empty() {
        return false;
    }
    let Some(const_expr) = time_recv else { return false };
    let ExprNode::Const { path } = &*const_expr.node else {
        return false;
    };
    path.last().map(|s| s.as_str()) == Some("Time")
}

/// Recognize `recv.is_a?(Const)` and return `(recv, last-segment of
/// Const path)` so the caller can splice in Go's type-assert
/// if-init form. Returns `None` for any other shape, including
/// `is_a?(SomeRuntimeValue)` (rare; emit falls back to the generic
/// `IsAP` send and the gap stays visible).
fn is_a_predicate<'a>(e: &'a Expr) -> Option<(&'a Expr, &'a str)> {
    let ExprNode::Send { recv, method, args, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "is_a?" || args.len() != 1 {
        return None;
    }
    let r = recv.as_ref()?;
    let ExprNode::Const { path } = &*args[0].node else {
        return None;
    };
    Some((r, path.last()?.as_str()))
}

/// Map a Ruby class name to the Go type used by `v.(T)` assertion.
/// Returns `None` for user-defined classes — those fall through to
/// the bare-call emit so the gap stays visible. Hash / Array map to
/// their permissive `map[string]any` / `[]any` shapes because the
/// is_a? branch typically treats the asserted value generically (use
/// sites that need a tighter element Ty re-narrow at the use, not
/// here).
fn ruby_class_to_go_assert_ty(class: &str) -> Option<&'static str> {
    Some(match class {
        "Integer" => "int64",
        "Float" => "float64",
        "String" => "string",
        "Symbol" => "string",
        "Hash" => "map[string]any",
        "Array" => "[]any",
        _ => return None,
    })
}

/// Result of recognizing an `if recv.is_a?(Class)` shape and
/// mapping it to Go's type-assert if-init form. Carries everything
/// the per-If handler needs to splice in the init clause AND
/// substitute uses of the original receiver inside the then_branch.
pub(super) struct IsAInit {
    /// `s, ok := v.(string); ` — pre-cond text dropped into the
    /// `if`'s init slot. Ends with `; ` so the caller can concat
    /// straight onto the cond.
    pub init: String,
    /// Cond text — always `"ok"` today (the assertion's bool).
    pub cond: &'static str,
    /// Original receiver's Var name, if the receiver was a bare
    /// Var. The then_branch's child ctx adds this → asserted_ident
    /// to its rename map. `None` for non-Var receivers (`foo().is_a?`)
    /// — those emit the init but skip the rewrite.
    pub recv_name: Option<String>,
    /// Asserted identifier (`s`/`i`/`f`/...) — the new name bound
    /// in the if's scope.
    pub asserted_ident: &'static str,
}

/// Build the `IsAInit` for an `is_a?` predicate that has a mapped
/// Go assertion type. Returns `None` if either the shape or the
/// class isn't supported, so callers can fall through to the
/// unchanged path.
fn try_emit_is_a_init(ctx: &EmitCtx, cond: &Expr) -> Option<IsAInit> {
    let (recv, class) = is_a_predicate(cond)?;
    let go_ty = ruby_class_to_go_assert_ty(class)?;
    // Skip when the receiver's Go-emit shape ALREADY matches the
    // assertion target. Empty-as-nil maps `Union[Str, Nil]` to
    // `string`, so an `if s.is_a?(String)` against an already-
    // string `s` would emit `s.(string)` — Go rejects "type
    // assertion on non-interface". Returning None makes the
    // caller fall back to plain bool-cond emit.
    if super::ty::go_ty_stub(recv.ty.as_ref()) == go_ty {
        return None;
    }
    let recv_s = emit_expr(ctx, recv);
    let asserted_ident = assert_ident_for(go_ty);
    let recv_name = match &*recv.node {
        ExprNode::Var { name, .. } => Some(name.as_str().to_string()),
        _ => None,
    };
    Some(IsAInit {
        init: format!("{asserted_ident}, ok := {recv_s}.({go_ty}); "),
        cond: "ok",
        recv_name,
        asserted_ident,
    })
}

/// Pick a short, idiomatic Go identifier for the asserted typed
/// value. Single-letter conventions: `s` for strings, `i` for
/// ints, `f` for floats. Falls back to `narrowed` for anything
/// else (won't currently hit, but keeps the surface total).
fn assert_ident_for(go_ty: &str) -> &'static str {
    match go_ty {
        "string" => "s",
        "int64" => "i",
        "float64" => "f",
        "map[string]any" => "h",
        "[]any" => "a",
        _ => "narrowed",
    }
}

/// Result of recognizing the `return X if var.nil?` head of a
/// method body whose `var` has a `Union { Nil, T }` type — the rest
/// of the body sees `var` narrowed to `T`. Drives a runtime type
/// assertion + rename so subsequent uses see the typed value.
struct NilNarrow {
    /// Original Var name (`s` in `return … if s.nil?`).
    recv_name: String,
    /// Non-Nil Go assertion type the union collapsed to (`string`,
    /// `int64`, ...).
    go_ty: &'static str,
    /// New identifier bound to the asserted value (`s_str` for
    /// String-narrowed `s`).
    narrowed_ident: String,
}

/// Recognize `If { cond: var.nil?, then: Return, else: Nil }` at
/// the head of a `Seq` body and, when `var`'s Ty is a Union with
/// exactly one non-Nil variant we can map to Go, return the
/// narrowing plan. None when the shape doesn't match — caller
/// emits the head expr normally and the rest unchanged.
fn try_nil_narrow_head(first: &Expr) -> Option<NilNarrow> {
    let ExprNode::If { cond, then_branch, else_branch } = &*first.node else {
        return None;
    };
    if !is_nil_lit(else_branch) {
        return None;
    }
    // then_branch must be a Return (the early-out shape we're after).
    if !matches!(&*then_branch.node, ExprNode::Return { .. }) {
        return None;
    }
    let ExprNode::Send { recv, method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let r = recv.as_ref()?;
    let ExprNode::Var { name, .. } = &*r.node else {
        return None;
    };
    let Some(union_ty @ Ty::Union { variants }) = r.ty.as_ref() else {
        return None;
    };
    let non_nil: Vec<&Ty> = variants
        .iter()
        .filter(|t| !matches!(t, Ty::Nil))
        .collect();
    if non_nil.len() != 1 {
        return None;
    }
    let go_ty = match non_nil[0] {
        Ty::Str | Ty::Sym => "string",
        Ty::Int => "int64",
        Ty::Float => "float64",
        Ty::Bool => "bool",
        _ => return None,
    };
    // Skip when the union's Go-emit shape ALREADY matches the
    // narrow target: `Union[Str, Nil]` now renders as `string`
    // (empty-as-nil convention in go_ty_stub), so `s.(string)`
    // would be a no-op assertion on an already-string value —
    // Go rejects "type assertion on non-interface". The bare-
    // value narrow path (no init, no rename) is what's right
    // when the underlying var is already typed.
    if super::ty::go_ty_stub(Some(union_ty)) == go_ty {
        return None;
    }
    let short = match go_ty {
        "string" => "str",
        "int64" => "int",
        "float64" => "f64",
        "bool" => "b",
        _ => "v",
    };
    let recv_name = name.as_str().to_string();
    let narrowed_ident = format!("{recv_name}_{short}");
    Some(NilNarrow { recv_name, go_ty, narrowed_ident })
}

/// Emit a Ruby `x = value` as a Go assignment. Uses `:=` for the
/// common case (fresh local binding) and falls back to `=` when the
/// lvalue isn't a plain local. Targets covered:
///
/// - `LValue::Var` → `name := value`
/// - `LValue::Ivar` → `self.name = value` (instance receiver)
/// - other → emit just the value with a `/* TODO */` marker so the
///   gap is loudly visible in the v2/ output
fn emit_assign(ctx: &EmitCtx, target: &crate::expr::LValue, value: &Expr) -> String {
    use crate::expr::LValue;
    // Ruby's `if`/`case` are expressions; Go's are statements. When
    // an Assign's value is one of those expression-bearing
    // statement-shapes, wrap in an immediately-invoked closure so
    // the result lands in the bound variable. `interface{}` keeps
    // the return type total — caller-side uses see `interface{}`
    // and downstream emits (Sprintf %v) already accept that.
    let v = if matches!(&*value.node, ExprNode::If { .. } | ExprNode::Case { .. }) {
        // IIFE return type matches the value's analyzer-inferred Ty
        // (narrow primitives when both branches agree, `interface{}`
        // otherwise). Same rationale as the writer-suffix peephole
        // above — using the narrow type lets the resulting assignment
        // typecheck against typed slots without callsite assertion.
        let ret_ty = super::ty::go_ty_stub(value.ty.as_ref());
        let body = emit_return_body(ctx, value);
        let indented = body
            .lines()
            .map(|l| format!("\t{l}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("func() {ret_ty} {{\n{indented}\n}}()")
    } else {
        emit_expr(ctx, value)
    };
    match target {
        LValue::Var { name, .. } => {
            // First assignment to this name → declare (`:=`).
            // Subsequent assignments → reassign (`=`). Matches Ruby's
            // flat function scope: Ruby `x = 1` inside an if-block
            // reassigns the outer `x` rather than shadowing it, which
            // is what we want emitted in Go (otherwise the write is
            // silently lost when the inner scope exits). Sanitized
            // through the Go-keyword filter to match Var-read emit.
            //
            // Int-typed first declarations pin the var as `int64`:
            // Go's untyped-int-literal default is `int`, but Ruby
            // Integer maps to int64 in go_ty_stub. Without the pin,
            // patterns like `i = 0; while i < arr.length` (which
            // emits `int64(len(arr))`) hit a type mismatch between
            // `int` and `int64`.
            let name_s = super::library::sanitize(name.as_str());
            let first = ctx.declared.borrow_mut().insert(name_s.clone());
            if first {
                // Nil-init accumulator pattern: `result = nil` ahead
                // of a conditional `result = instance` and a tail
                // `return result`. Go's `:=` can't type-infer from
                // bare nil, and the analyzer's e.ty on Lit::Nil is
                // just Ty::Nil — which alone doesn't tell us the
                // accumulator's effective type. Use ctx.return_ty as
                // the heuristic: the lowerer's adapter_emit produces
                // these exclusively as return-value accumulators in
                // nilable-pointer-returning methods (_adapter_find_by_id,
                // _adapter_reload, belongs_to lookups). Falls back to
                // `interface{}` when no return type is available (the
                // var becomes untyped nil, downstream assigns may still
                // mismatch — kept as visible failure rather than
                // silent corruption).
                if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                    let ty_str = ctx
                        .return_ty
                        .as_ref()
                        .map(|t| super::ty::go_ty_stub(Some(t)))
                        .unwrap_or_else(|| "interface{}".to_string());
                    return format!("var {name_s} {ty_str} = nil");
                }
                if matches!(value.ty, Some(Ty::Int)) {
                    format!("var {name_s} int64 = {v}")
                } else {
                    format!("{name_s} := {v}")
                }
            } else {
                format!("{name_s} = {v}")
            }
        }
        LValue::Ivar { name } => {
            // Same scoping rule as Ivar reads: module-singleton
            // writes target the namespaced `<Class>_<ivar>_slot`
            // package var; bare class methods write to the bare
            // lowercase package var; instance methods write through
            // `self.Field` (PascalCased).
            if ctx.in_module_singleton {
                if let Some(class) = ctx.class_name.as_deref() {
                    format!("{class}_{}_slot = {v}", name.as_str())
                } else {
                    format!("{} = {v}", name.as_str())
                }
            } else if ctx.in_class_method {
                format!("{} = {v}", name.as_str())
            } else {
                let pascal = go_field_name(name.as_str());
                format!("self.{pascal} = {v}")
            }
        }
        _ => crate::emit::diagnostics::report_unsupported(value.span, "go2", "Assign-target", ""),
    }
}

/// Ruby method names that map to Go binary operators when called
/// with a receiver and one argument. `nil` semantics for `==` differ
/// (Go nil interface vs typed nil) but that's a downstream concern —
/// at this level we just rewrite the call shape.
fn binary_op(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "==",
        "!=" => "!=",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        _ => return None,
    })
}

/// AR/stdlib method names that should emit with parens on a model
/// struct receiver. Everything else on a non-Class receiver with no
/// args is treated as a field read. Grows alongside the runtime.
/// Identify Ruby attr-writer method names (`x=`, `name=`) so the
/// Send peephole rewrites them to Go field assignment. Excludes
/// comparison operators (`==`, `!=`, `<=`, `>=`, `<=>`) and the
/// indexed-setter operator (`[]=`, handled separately via OpSet)
/// — those end with `=` but are NOT attr writers. A real writer's
/// last-before-`=` character must be an identifier char (letter,
/// digit, or `_`).
fn is_writer_method_name(name: &str) -> bool {
    if !name.ends_with('=') || name.len() < 2 {
        return false;
    }
    let prev = name.as_bytes()[name.len() - 2] as char;
    prev.is_ascii_alphanumeric() || prev == '_'
}

/// Detect a `self.render(...)` call where the lowerer left kwargs as
/// a trailing Hash literal, and return the padded args list matching
/// the Go-side `(*ActionControllerBase).Render(body, status,
/// content_type, location string)` 4-positional signature. Returns
/// None when the call shape doesn't match (different recv, wrong
/// method, no trailing Hash AND arg-count already matches the 4).
///
/// Default: status `"ok"` (per Ruby `status: :ok`), content_type and
/// location both `""`. Matches the Ruby source semantics —
/// `runtime/ruby/action_controller/base.rb`'s `def render(body,
/// status: :ok, content_type: nil, location: nil)`. The rust2 target
/// has a generalized `GLOBAL_CLASS_METHOD_DEFAULTS` registry; go2 uses
/// this hardcoded peephole until a forcing third case appears.
fn try_expand_render_kwargs(
    ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<Vec<String>> {
    if method != "render" {
        return None;
    }
    // `self.render(...)` only — Const-recv would route through a
    // different bare-fn path and isn't the pattern controllers hit.
    let Some(r) = recv else { return None };
    if !matches!(&*r.node, ExprNode::SelfRef) {
        return None;
    }
    // Pull body + kwargs from args.
    let body_s = match args.first() {
        Some(a) => emit_expr(ctx, a),
        None => return None,
    };
    let mut status_s = "\"ok\"".to_string();
    let mut content_type_s = "\"\"".to_string();
    let mut location_s = "\"\"".to_string();
    if args.len() >= 2 {
        // Look for a trailing Hash literal carrying status/content_type/
        // location kwargs. Hash literal entries are (key_expr, value_expr);
        // match keys by Lit::Str / Lit::Sym contents.
        if let ExprNode::Hash { entries, .. } = &*args[1].node {
            for (k, v) in entries {
                let key_text = match &*k.node {
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                    _ => None,
                };
                let v_s = emit_expr(ctx, v);
                match key_text.as_deref() {
                    Some("status") => status_s = v_s,
                    Some("content_type") => content_type_s = v_s,
                    Some("location") => location_s = v_s,
                    _ => {}
                }
            }
        }
    }
    Some(vec![body_s, status_s, content_type_s, location_s])
}

/// Sibling of `try_expand_render_kwargs` for `self.redirect_to(...)`.
/// Ruby `redirect_to(path, notice: ..., alert: ..., status: ...)` →
/// Go `(*ActionControllerBase).RedirectTo(path, notice, alert, status
/// string)`. Same hardcoded peephole rationale as render — pending
/// the GLOBAL_CLASS_METHOD_DEFAULTS-equivalent registry build-out.
fn try_expand_redirect_to_kwargs(
    ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<Vec<String>> {
    if method != "redirect_to" {
        return None;
    }
    let Some(r) = recv else { return None };
    if !matches!(&*r.node, ExprNode::SelfRef) {
        return None;
    }
    let path_s = match args.first() {
        Some(a) => emit_expr(ctx, a),
        None => return None,
    };
    let mut notice_s = "\"\"".to_string();
    let mut alert_s = "\"\"".to_string();
    // Ruby `def redirect_to(path, ..., status: :found)` — when the
    // call site doesn't pass an explicit status:, the framework
    // default is :found (302). Match that here so POST→create
    // redirects respond 302 instead of the empty-string-resolves-to-
    // 200 fallback ResolveStatus produces.
    let mut status_s = "\"found\"".to_string();
    if args.len() >= 2 {
        if let ExprNode::Hash { entries, .. } = &*args[1].node {
            for (k, v) in entries {
                let key_text = match &*k.node {
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                    _ => None,
                };
                let v_s = emit_expr(ctx, v);
                match key_text.as_deref() {
                    Some("notice") => notice_s = v_s,
                    Some("alert") => alert_s = v_s,
                    Some("status") => status_s = v_s,
                    _ => {}
                }
            }
        }
    }
    Some(vec![path_s, notice_s, alert_s, status_s])
}

/// Sibling of `try_expand_render_kwargs` for `self.head(...)`. Ruby
/// `head :status` / `head :status, content_type: "..."` lowers to a
/// `self.Head(status, ...kwargs_or_nothing)` Send. The Go shim
/// signature is `Head(status string, content_type string)` — two
/// positional strings. Two call shapes hit this peephole:
///
/// - `self.head(:not_found)` → `self.Head("not_found", "")`
/// - `self.head(:no_content, content_type: "application/json")` →
///   `self.Head("no_content", "application/json")`
///
/// The bare-1-arg shape was previously emitting `self.Head("not_found")`
/// (arity mismatch); the kwargs shape was emitting the Hash literal
/// as the second positional (type mismatch). Same hardcoded peephole
/// rationale as render/redirect_to — pending the
/// GLOBAL_CLASS_METHOD_DEFAULTS-equivalent registry build-out.
fn try_expand_head_kwargs(
    ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<Vec<String>> {
    if method != "head" {
        return None;
    }
    let Some(r) = recv else { return None };
    if !matches!(&*r.node, ExprNode::SelfRef) {
        return None;
    }
    let status_s = match args.first() {
        Some(a) => emit_expr(ctx, a),
        None => return None,
    };
    let mut content_type_s = "\"\"".to_string();
    if args.len() >= 2 {
        if let ExprNode::Hash { entries, .. } = &*args[1].node {
            for (k, v) in entries {
                let key_text = match &*k.node {
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                    _ => None,
                };
                let v_s = emit_expr(ctx, v);
                if key_text.as_deref() == Some("content_type") {
                    content_type_s = v_s;
                }
            }
        }
    }
    Some(vec![status_s, content_type_s])
}

/// Sibling for `ActionView::ViewHelpers.truncate(s, length:, omission:)`.
/// Ruby `truncate(article.body, length: 100)` is a Const-recv call (not
/// `self.X` like render/head/redirect_to). The Go emit signature is
/// `truncate(s string, _opts ...interface{})` which type-asserts
/// `_opts[0].(int64)` then `_opts[1].(string)` — so the call site must
/// pass typed positionals, not a Hash. Without this peephole the call
/// emitted `truncate(s, map[string]int64{"length": 100})` which panicked
/// at runtime on the int64 type assertion.
///
/// Emits only as many positional opts as the Hash literal actually
/// carries (length only → 1 positional; both → 2). The Ruby-source
/// defaults (30 / "...") are intentionally NOT filled in here — the
/// emit body's `if len(_opts) > N` guard preserves the call-arity
/// semantic that "omitted = use the runtime body's fallback".
fn try_expand_truncate_kwargs(
    _ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<Vec<String>> {
    if method != "truncate" {
        return None;
    }
    let r = recv?;
    let ExprNode::Const { path } = &*r.node else {
        return None;
    };
    if path.last().map(|s| s.as_str()) != Some("ViewHelpers") {
        return None;
    }
    let s_arg = emit_expr(_ctx, args.first()?);
    let mut length_s: Option<String> = None;
    let mut omission_s: Option<String> = None;
    if let Some(arg2) = args.get(1) {
        if let ExprNode::Hash { entries, .. } = &*arg2.node {
            for (k, v) in entries {
                let key_text = match &*k.node {
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                    _ => None,
                };
                let v_s = emit_expr(_ctx, v);
                match key_text.as_deref() {
                    Some("length") => length_s = Some(format!("int64({})", v_s)),
                    Some("omission") => omission_s = Some(v_s),
                    _ => {}
                }
            }
        }
    }
    // Pad missing kwargs with the Ruby-source defaults (length 30,
    // omission "..."). The emitted function body's `_opts[0].(int64)` /
    // `_opts[1].(string)` assertions would zero out (0 / "") when an
    // arg is missing — which divergence from Rails' truncate manifests
    // as a 1-3 char text-diff on every truncated paragraph (the
    // omission length isn't subtracted from the cutoff, so the visible
    // text runs over by `len("...")` chars). Pad here so the function
    // body sees the right defaults via the typed assertions.
    let mut out = vec![s_arg];
    match (length_s, omission_s) {
        (Some(l), Some(o)) => { out.push(l); out.push(o); }
        (Some(l), None) => { out.push(l); out.push("\"...\"".to_string()); }
        (None, Some(o)) => { out.push("int64(30)".to_string()); out.push(o); }
        (None, None) => { out.push("int64(30)".to_string()); out.push("\"...\"".to_string()); }
    }
    Some(out)
}

/// `Broadcasts.{append,prepend,replace,remove}` call detection — the
/// hand-written `runtime/go/v2/broadcasts.go` shims take
/// `map[string]interface{}` for the kwargs bag. Used by the Hash-
/// widening peephole at args_s emit so the call site widens
/// uniform-string Hash literals to interface-valued maps before
/// `Broadcasts_X(...)` argument typecheck.
fn is_broadcasts_call(recv: Option<&Expr>, method: &str) -> bool {
    if !matches!(method, "append" | "prepend" | "replace" | "remove") {
        return false;
    }
    let Some(r) = recv else { return false };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    path.last().map(|s| s.as_str()) == Some("Broadcasts")
}

/// `ActionView::ViewHelpers` helpers whose hand-written runtime
/// signature takes `map[string]interface{}` but whose call sites in
/// the layout view pass a uniform-string Hash literal (e.g.
/// `stylesheet_link_tag("application", data: { turbo_track: "reload" })`).
/// Widen the Hash arg to interface-valued so the call typechecks.
fn is_view_helpers_widen_call(recv: Option<&Expr>, method: &str) -> bool {
    if !matches!(
        method,
        "stylesheet_link_tag" | "javascript_importmap_tags"
    ) {
        return false;
    }
    let Some(r) = recv else { return false };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    path.last().map(|s| s.as_str()) == Some("ViewHelpers")
}

/// Ruby names of the methods listed in the Modeler interface
/// (`runtime/go/v2/modeler.go`). When a Base method body calls
/// `self.class.X` or `self.X` against one of these, the emit must
/// route through `self.Self.X()` for polymorphic dispatch — otherwise
/// the static-bound `*ActiveRecordBase` method (panic stub) fires
/// instead of the subclass override. Keep in sync with the Modeler
/// interface declaration; adding a member here requires also adding
/// a method shim per AR subclass.
fn is_modeler_member(name: &str) -> bool {
    matches!(
        name,
        "schema_columns"
            | "op_get"
            | "op_set"
            | "[]"
            | "[]="
            // Adapter primitives — Save() calls `self._adapter_insert`,
            // `self._adapter_update`, `self._adapter_delete`; each AR
            // subclass overrides with a real SQL-emitting body.
            | "_adapter_insert"
            | "_adapter_update"
            | "_adapter_delete"
            // Validation + callbacks — Save() drives `valid?` →
            // `self.validate`; create/update/destroy paths drive
            // `after_*_commit` and `before_destroy`. Subclasses (Article,
            // Comment) define the real bodies; Base's stubs are empty.
            | "validate"
            | "before_destroy"
            | "after_create_commit"
            | "after_update_commit"
            | "after_destroy_commit"
    )
}

fn is_known_go_method(name: &str) -> bool {
    matches!(
        name,
        "save" | "save!" | "destroy" | "destroy!" | "update" | "update!"
            | "delete" | "touch" | "reload"
            | "validate" | "attributes"
        // `errors` was here but is now a Base struct field
        // (`*ActiveRecordBase.Errors []string`) reached via Go method
        // promotion through embedding (Q1 wedge). It collapses out
        // of method emit via the trivial-ivar-reader collision check;
        // call sites should emit as `self.Errors` (field read), not
        // `self.Errors()` — keeping it in the force-parens list
        // produced invalid Go: `self.Errors() = append(self.Errors(), X)`.
    )
}

/// Methods that emit as a Go method call (`.X()`) rather than the
/// default attr-reader-shaped bare field read (`.X`) when called
/// 0-arg on a Class-typed receiver. Real methods (not attr_reader-
/// backed struct fields) — without parens, `go vet` flags the emit
/// as a method-value reference. Replace with a per-class method
/// registry once go2 grows one; this list is the minimum needed
/// to keep `runtime/ruby/` emit clean today.
/// Inline `ActionView::ViewHelpers.dom_id(record [, suffix])` when
/// the record's Ty is a concrete model. The runtime helper takes
/// `*ActiveRecordBase` which Go won't accept a `*Article` for —
/// inlining sidesteps the upcast problem entirely. Suffix arg is
/// inlined as a literal symbol/string. Returns None on any shape
/// that doesn't match (non-Const recv, recv not the ViewHelpers
/// module, non-Class-typed record, missing args, dynamic suffix).
fn try_dom_id_inline(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    args_s: &[String],
) -> Option<String> {
    if method != "dom_id" {
        return None;
    }
    let r = recv?;
    let ExprNode::Const { path } = &*r.node else { return None };
    let last = path.last().map(|s| s.as_str())?;
    if last != "ViewHelpers" {
        return None;
    }
    if args.is_empty() || args.len() > 2 {
        return None;
    }
    let record = &args[0];
    let class_name = match record.ty.as_ref()? {
        Ty::Class { id, .. } => id.0.as_str().to_string(),
        _ => return None,
    };
    if class_name == "Base" || class_name == "ActiveRecord::Base" {
        return None;
    }
    let prefix = crate::naming::snake_case(
        class_name.rsplit("::").next().unwrap_or(class_name.as_str()),
    );
    let record_s = &args_s[0];
    if args.len() == 1 {
        return Some(format!(
            "fmt.Sprintf(\"{}_%v\", {}.ID)",
            prefix, record_s,
        ));
    }
    // 2-arg form: suffix is typically a literal `:comments_count`.
    let suffix = &args[1];
    let suffix_lit = match &*suffix.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => value.as_str().to_string(),
        ExprNode::Lit {
            value: Literal::Str { value },
        } => value.as_str().to_string(),
        _ => return None,
    };
    // Ruby `"#{suffix}_#{record.dom_prefix()}_#{record.id}"` —
    // suffix BEFORE prefix in the resulting id. Rails convention:
    // `dom_id(article, :comments_count)` → "comments_count_article_3"
    // (not "article_3_comments_count"). See runtime/ruby/action_view/
    // view_helpers.rb#dom_id.
    Some(format!(
        "fmt.Sprintf(\"{}_{}_%v\", {}.ID)",
        suffix_lit, prefix, record_s,
    ))
}

fn is_known_class_method(name: &str) -> bool {
    matches!(
        name,
        // ActiveRecord::Base lowerer-synthesized panic-overridden
        // per-model method — view_helpers' dom_id relies on parens.
        "dom_prefix"
        // AR::Base instance method that subclasses inherit via Go
        // embedding (`Article` embeds `*ApplicationRecord` →
        // `*ActiveRecordBase`). The 0-arg call site
        // `instance.mark_persisted!` defaults to bare-field-read
        // shape; force parens so Go method-call promotion fires.
        | "mark_persisted!"
        // `has_many` / `belongs_to` relation methods. The lowerer
        // synthesizes per-model accessor methods (`Article#comments`
        // returns `[]*Comment`, `Comment#article` returns `*Article`)
        // that views and controllers read at non-self sites. Without
        // parens these emit as method-value references, which Go's
        // `len()`, `for range`, and assignment all reject.
        | "comments" | "article"
    )
}

/// Map Ruby String methods onto Go expressions that compile. `strip`
/// in Ruby is `strings.TrimSpace(s)` in Go — no method form exists.
/// Returns `Some(emit_text)` for a handled method. Unhandled methods
/// fall through to the default `.Method()` emit which may or may not
/// compile depending on the target receiver's actual methods.
fn map_go_str_method(method: &str, recv_text: &str) -> Option<String> {
    match method {
        "strip" => Some(format!("strings.TrimSpace({recv_text})")),
        "upcase" => Some(format!("strings.ToUpper({recv_text})")),
        "downcase" => Some(format!("strings.ToLower({recv_text})")),
        // Ruby `s.to_i` ignores leading whitespace and trailing
        // non-numeric, returns 0 on parse failure. Mirror that
        // shape with strconv.Atoi + zero fallback via IIFE. Used
        // heavily by controllers extracting `params[:id]` into a
        // numeric for `Article.find`. The `fmt.Sprintf("%v", ...)`
        // wrap lets it cope with `params[k]` which is interface{}
        // rather than a typed string — without it the call site
        // type-asserts to string before TrimSpace and `go vet` fails.
        "to_i" => Some(format!(
            "func() int64 {{ n, _ := strconv.ParseInt(strings.TrimSpace(fmt.Sprintf(\"%v\", {recv_text})), 10, 64); return n }}()"
        )),
        "to_f" => Some(format!(
            "func() float64 {{ n, _ := strconv.ParseFloat(strings.TrimSpace(fmt.Sprintf(\"%v\", {recv_text})), 64); return n }}()"
        )),
        _ => None,
    }
}

/// Single-argument Ruby string methods → Go stdlib. Receiver must be
/// `Ty::Str` for these to apply (caller-side gate).
fn map_go_str_method_1arg(method: &str, recv: &str, arg: &str) -> Option<String> {
    match method {
        "split" => Some(format!("strings.Split({recv}, {arg})")),
        "start_with?" => Some(format!("strings.HasPrefix({recv}, {arg})")),
        "end_with?" => Some(format!("strings.HasSuffix({recv}, {arg})")),
        "include?" => Some(format!("strings.Contains({recv}, {arg})")),
        _ => None,
    }
}

pub(super) fn emit_block_body(ctx: &EmitCtx, e: &Expr) -> String {
    let raw = match &*e.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|sub| emit_expr(ctx, sub))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_expr(ctx, e),
    };
    raw.lines().map(|l| format!("\t{l}")).collect::<Vec<_>>().join("\n")
}

/// Render a `.map { |x| body }` block body: leading exprs in a Seq
/// emit as statements, the tail expr feeds `out = append(out, …)`.
/// A single non-Seq body is the tail. Two tabs of indent — one for
/// the IIFE, one for the `for` loop.
pub(super) fn emit_map_block_body(ctx: &EmitCtx, e: &Expr) -> String {
    let (stmts, tail) = match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let last = exprs.len() - 1;
            let stmts: Vec<String> = exprs[..last]
                .iter()
                .map(|sub| emit_expr(ctx, sub))
                .collect();
            (stmts, emit_expr(ctx, &exprs[last]))
        }
        _ => (Vec::new(), emit_expr(ctx, e)),
    };
    let mut lines: Vec<String> = stmts;
    lines.push(format!("out = append(out, {tail})"));
    lines
        .iter()
        .flat_map(|s| s.lines().map(|l| format!("\t\t{l}")).collect::<Vec<_>>())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => go_str_literal(value),
        Literal::Sym { value } => go_str_literal(value.as_str()),
        Literal::Regex { pattern, flags } => {
            // Empty flags → bare pattern (Go's `(?)` is a parse
            // error; the `(?<flags>)` group requires at least one
            // flag char). With flags, prepend the standard prefix.
            let go_pattern = ruby_regex_to_go(pattern);
            let full = if flags.is_empty() {
                go_pattern
            } else {
                format!("(?{flags}){go_pattern}")
            };
            format!("regexp.MustCompile({})", go_str_literal(&full))
        }
    }
}

/// Translate Ruby regex source to a Go-acceptable pattern. The two
/// regression points hit so far:
///
/// - `\b` / `\f` inside a character class — Ruby treats these as
///   backspace / form-feed (control chars); Go's regexp rejects them
///   as "invalid escape sequence" inside `[]` (interprets `\b` only
///   as word boundary, valid only outside `[]`). Rewrite both to
///   the explicit hex form (`\x08` / `\x0c`) when seen inside `[]`.
///
/// Other Ruby regex extensions (named captures, lookbehind, etc.)
/// pass through unchanged for now; surface when they show up.
fn ruby_regex_to_go(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if in_class && next == 'b' {
                        out.push_str("\\x08");
                        chars.next();
                        continue;
                    }
                    if in_class && next == 'f' {
                        out.push_str("\\x0c");
                        chars.next();
                        continue;
                    }
                    out.push('\\');
                    out.push(next);
                    chars.next();
                } else {
                    out.push('\\');
                }
            }
            '[' => {
                in_class = true;
                out.push('[');
            }
            ']' => {
                in_class = false;
                out.push(']');
            }
            other => out.push(other),
        }
    }
    out
}

/// Emit a Go-syntactic double-quoted string literal. Rust's `{:?}`
/// produces `\u{8}` / `\u{c}` for backspace / form-feed which Go
/// rejects (`U+007B '{' illegal in escape sequence`); Go uses `\b`
/// `\f` plus the fixed-width `\xHH` / `\uHHHH` / `\UHHHHHHHH`
/// forms. Covers all controls + the standard escapable chars.
fn go_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{07}' => out.push_str("\\a"),
            '\u{0b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x7F => out.push(c),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Emit `expr` at body (return) position — Ruby's last-expression
/// semantics mapped to Go's explicit `return`. Recurses into `If`
/// and `Seq` so the return lands at the value-producing leaf. All
/// other variants emit as `return <value_expression>`.
///
/// Output is indented one tab in (caller wraps in `func ... { ... }`).
pub(super) fn emit_return_body(ctx: &EmitCtx, e: &Expr) -> String {
    let mut out = String::new();
    emit_return_at(ctx, e, &mut out, 1);
    out
}

/// Emit `e` in expression position — wraps statement-shaped IR
/// (If / Case) in an IIFE so it can stand in for a value (Send arg,
/// Hash value, format arg, ...). Falls through to `emit_expr` for
/// nodes that already produce expression-form Go. Matches the
/// IIFE-wrap pattern the Assign emit applies for the same nodes.
pub(super) fn emit_expr_as_value(ctx: &EmitCtx, e: &Expr) -> String {
    if matches!(&*e.node, ExprNode::If { .. } | ExprNode::Case { .. }) {
        let ret_ty = super::ty::go_ty_stub(e.ty.as_ref());
        let body = emit_return_body(ctx, e);
        let indented = body
            .lines()
            .map(|l| format!("\t{l}"))
            .collect::<Vec<_>>()
            .join("\n");
        return format!("func() {ret_ty} {{\n{indented}\n}}()");
    }
    emit_expr(ctx, e)
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

/// Wrap or replace the emitted return-value string when the
/// function's declared return type doesn't match what Go would
/// infer from the value expression.
///
/// Cases handled:
/// - Ty::Int return + non-nil value → `int64(v)`. Ruby `n = 0;
///   ...; return n` lowers to `n := 0` (Go infers `int`); bare
///   `return n` against an `int64` signature is a compile error.
///   The wrap is a no-op for already-int64 values.
/// - `Union[Str, Nil]` return (renders as Go `string`) + bare
///   `nil` value → `""`. Ruby `return nil` against `String?` is
///   valid; under the empty-as-nil convention Go needs `""`.
/// Recover (key_go_ty, value_go_ty) for a receiver Ty that should
/// be a Hash. Falls back to `("string", "any")` when the analyzer
/// gave no useful info — that's the dominant Ruby Hash shape
/// (string keys, untyped values) and matches what Go's `map[string]
/// any` would be if the analyzer had set the Ty. Nested Unions are
/// flattened via `union_non_nil_core` so `Union[Hash, Nil]` (a
/// nullable map) carries through.
/// Predicate for the fetch peephole's default-arg cast. Returns true
/// when the receiver's value type is the `any`-alias `RoundhouseParamValue`
/// (or bare `any` / `interface{}`) AND the default's emitted shape isn't
/// already that type. The cast widens concrete defaults (`map[…]…{}`,
/// `[]…{}`, `""`, `int64(0)`) into the receiver's value-type slot so
/// `cmp.Or`'s generic inference unifies both args under T=any.
fn default_needs_v_ty_cast(v_ty: &str, default_s: &str) -> bool {
    if !matches!(v_ty, "RoundhouseParamValue" | "any" | "interface{}") {
        return false;
    }
    // The default already names the v_ty (`any(...)` / `interface{}(...)`
    // / `RoundhouseParamValue(...)`) — leave it alone.
    let trimmed = default_s.trim_start();
    !(trimmed.starts_with(&format!("{v_ty}("))
        || trimmed.starts_with("any(")
        || trimmed.starts_with("interface{}("))
}

fn hash_kv_go_tys(ty: Option<&Ty>) -> (String, String) {
    let core = match ty {
        Some(t) => union_non_nil_core(t).unwrap_or(t),
        None => return ("string".to_string(), "any".to_string()),
    };
    match core {
        Ty::Hash { key, value } => (
            super::ty::go_ty_stub(Some(key)),
            super::ty::go_ty_stub(Some(value)),
        ),
        _ => ("string".to_string(), "any".to_string()),
    }
}

/// Flatten nested `Union[Union[T, Nil], Nil]` (and similar)
/// down to its non-Nil core. Returns None when the union has
/// multiple non-Nil variants (genuine sum type, not a nullable
/// wrapper). The analyzer sometimes double-wraps nullable Tys
/// when a field's declared `String?` is re-narrowed in flow;
/// the emit needs the structural answer, not the literal Union
/// shape.
/// True when the receiver's analyzer-set Ty renders to `interface{}`
/// via `go_ty_stub` (Untyped / Var / Bottom / multi-variant Union /
/// no Ty set). Used by the `.each` peephole to decide whether to
/// inject a type assertion (`recv.(map[string]any)` / `.([]any)`)
/// before the range — Go can't range over `interface{}` directly.
fn recv_ty_renders_as_interface(recv: &Expr) -> bool {
    super::ty::go_ty_stub(recv.ty.as_ref()) == "interface{}"
}

/// Identity helper so `literal_kind_str("string")` reads in the
/// same shape as `uniform_value_literal_kind` below — both feed
/// into the `(k_ty, v_ty)` tuple build in the Hash literal fallback.
fn literal_kind_str(s: &'static str) -> &'static str {
    s
}

/// Walk a Hash literal's entry list. When every value is the same
/// primitive literal kind, return the Go type for that kind so the
/// fallback Hash emit pins it (`map[string]int64` for STATUS_CODES,
/// `map[string]string` for HTML_ESCAPES). Returns `None` for mixed
/// kinds, empty entries, or any non-primitive value shape (nested
/// Hash, Array, Send-result, ...) — those land on the
/// `map[string]interface{}` fallback.
fn uniform_value_literal_kind(entries: &[(Expr, Expr)]) -> Option<&'static str> {
    // Empty Hash `{}` keeps the legacy `map[string]string` default
    // (router.go's `params = {}` then string-keyed `params[k] = v`
    // accumulation rides on this — the empty-as-string heuristic
    // covers the no-context case better than `interface{}` would).
    if entries.is_empty() {
        return Some("string");
    }
    let mut iter = entries.iter();
    let first = iter.next()?;
    let go_ty = literal_to_go_ty(&first.1)?;
    for (_, v) in iter {
        if literal_to_go_ty(v) != Some(go_ty) {
            return None;
        }
    }
    Some(go_ty)
}

/// Map a single literal Expr to the Go type its value uses. Returns
/// `None` for non-literal Exprs or literal kinds without a clean Go
/// type mapping (Nil, Regex, ...).
fn literal_to_go_ty(e: &Expr) -> Option<&'static str> {
    let ExprNode::Lit { value } = &*e.node else { return None };
    Some(match value {
        Literal::Str { .. } | Literal::Sym { .. } => "string",
        Literal::Int { .. } => "int64",
        Literal::Float { .. } => "float64",
        Literal::Bool { .. } => "bool",
        _ => return None,
    })
}

/// True when the receiver's analyzer-set Ty is a `Class { .. }` —
/// either directly or wrapped in `Union<Class, Nil>` (a nullable
/// class field). Drives the `[]` / `[]=` peephole's decision to
/// dispatch through `.OpGet` / `.OpSet` instead of Go index syntax
/// (structs aren't indexable). `Hash`/`Array`/`Untyped` receivers
/// fall through to the bare-index emit, matching their Go shape.
fn recv_ty_is_class(ty: Option<&Ty>) -> bool {
    let Some(t) = ty else { return false };
    let core = union_non_nil_core(t).unwrap_or(t);
    matches!(core, Ty::Class { .. })
}

fn union_non_nil_core(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Union { variants } => {
            let non_nil: Vec<&Ty> = variants
                .iter()
                .filter(|t| !matches!(t, Ty::Nil))
                .collect();
            if non_nil.len() == 1 {
                union_non_nil_core(non_nil[0])
            } else {
                None
            }
        }
        other => Some(other),
    }
}

fn coerce_return_value(ctx: &EmitCtx, v: String) -> String {
    match ctx.return_ty.as_ref() {
        Some(Ty::Int) if v != "nil" => format!("int64({v})"),
        Some(Ty::Union { variants }) if v == "nil" => {
            let non_nil: Vec<&Ty> = variants
                .iter()
                .filter(|t| !matches!(t, Ty::Nil))
                .collect();
            match non_nil.as_slice() {
                [Ty::Str] | [Ty::Sym] => "\"\"".to_string(),
                _ => v,
            }
        }
        _ => v,
    }
}

fn emit_return_at(ctx: &EmitCtx, e: &Expr, out: &mut String, depth: usize) {
    match &*e.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            // Both branches enter fresh declared scopes. Without
            // this, sibling if-blocks each declaring the same var
            // would emit one as `:=` and the rest as `=` against
            // out-of-scope bindings (Flash#delete shape).
            let (init, cond_s, then_ctx) = match try_emit_is_a_init(ctx, cond) {
                Some(IsAInit { init, cond, recv_name, asserted_ident }) => {
                    let child = match recv_name {
                        Some(n) => ctx.enter_scope().with_rename(n, asserted_ident.to_string()),
                        None => ctx.enter_scope(),
                    };
                    (init, cond.to_string(), child)
                }
                None => (String::new(), emit_expr(ctx, cond), ctx.enter_scope()),
            };
            indent(out, depth);
            out.push_str(&format!("if {init}{cond_s} {{\n"));
            emit_return_at(&then_ctx, then_branch, out, depth + 1);
            // Skip the else clause when it's an implicit nil — the
            // `return X if cond` shape doesn't want a `nil` else.
            if !is_nil_lit(else_branch) {
                indent(out, depth);
                out.push_str("} else {\n");
                let else_ctx = ctx.enter_scope();
                emit_return_at(&else_ctx, else_branch, out, depth + 1);
            }
            indent(out, depth);
            out.push_str("}\n");
        }
        ExprNode::Seq { exprs } => {
            // All but the last are statements (effects-only); last is
            // the return-position expression. Special-case: when the
            // first expr is `return X if var.nil?` and `var`'s Ty is
            // `Union { Nil, T }`, narrow `var` to `T` for the rest
            // of the Seq via runtime type assertion + rename.
            let narrow = exprs.first().and_then(try_nil_narrow_head);
            let mut tail_ctx_cell: Option<EmitCtx> = None;
            for (i, sub) in exprs.iter().enumerate() {
                let is_last = i + 1 == exprs.len();
                // Switch to the narrowed child ctx for everything
                // after the early-return head, once we've emitted it.
                let active_ctx = if i == 0 || tail_ctx_cell.is_none() {
                    ctx
                } else {
                    tail_ctx_cell.as_ref().unwrap()
                };
                if is_last {
                    emit_return_at(active_ctx, sub, out, depth);
                } else {
                    indent(out, depth);
                    out.push_str(&emit_expr(active_ctx, sub));
                    out.push('\n');
                }
                // After emitting the first expr, if it matched the
                // nil-narrow shape, inject the assertion + build the
                // child ctx that swaps `var` for the narrowed ident
                // in subsequent walks.
                if i == 0 {
                    if let Some(n) = &narrow {
                        indent(out, depth);
                        out.push_str(&format!(
                            "{narrowed} := {recv}.({go_ty})\n",
                            narrowed = n.narrowed_ident,
                            recv = n.recv_name,
                            go_ty = n.go_ty,
                        ));
                        // Track the newly-declared name so a later
                        // Assign to it emits `=`, not `:=`.
                        ctx.declared.borrow_mut().insert(n.narrowed_ident.clone());
                        tail_ctx_cell = Some(
                            ctx.with_rename(n.recv_name.clone(), n.narrowed_ident.clone()),
                        );
                    }
                }
            }
        }
        ExprNode::Return { value } => {
            // Already a return; don't double up to `return return X`.
            // Void methods elide the value entirely.
            if ctx.void_method {
                indent(out, depth);
                out.push_str("return\n");
            } else {
                let v = emit_expr(ctx, value);
                let v = coerce_return_value(ctx, v);
                indent(out, depth);
                out.push_str(&format!("return {v}\n"));
            }
        }
        ExprNode::While { .. } => {
            // While at body position emits as a statement, not
            // wrapped in `return`. Ruby's `while` evaluates to nil
            // so the loop's value is discarded; subsequent
            // statements (or an implicit nil tail) supply the
            // function's return.
            let s = emit_expr(ctx, e);
            for line in s.lines() {
                indent(out, depth);
                out.push_str(line);
                out.push('\n');
            }
        }
        // `raise X, "msg"` at body position — emit as `panic(...)`
        // statement without a `return` wrap. Ruby `raise` diverges
        // (Never type), so the method exits without a return value;
        // Go's `panic()` returns nothing, making `return panic(...)`
        // a syntax error. The Send arm in emit_expr handles the
        // expression-position case (rare — raise in value position).
        ExprNode::Send { recv: None, method, .. }
            if method.as_str() == "raise" =>
        {
            let s = emit_expr(ctx, e);
            indent(out, depth);
            out.push_str(&s);
            out.push('\n');
        }
        // `@ivar = value` (or `x = value`) at non-void tail position.
        // Ruby's assignment evaluates to the rhs, but Go disallows
        // assign-as-expression — `return slot = value` is a syntax
        // error. Emit the assign as a statement, then `return` by
        // re-reading the target. Read-back is safe for Ivar/Var and
        // avoids double-evaluating any side-effectful rhs.
        ExprNode::Assign { target, value } if !ctx.void_method => {
            use crate::expr::LValue;
            let assign_s = emit_assign(ctx, target, value);
            indent(out, depth);
            out.push_str(&assign_s);
            out.push('\n');
            let ret = match target {
                LValue::Ivar { name } => {
                    if ctx.in_module_singleton {
                        if let Some(class) = ctx.class_name.as_deref() {
                            format!("{class}_{}_slot", name.as_str())
                        } else {
                            name.as_str().to_string()
                        }
                    } else if ctx.in_class_method {
                        name.as_str().to_string()
                    } else {
                        format!("self.{}", go_field_name(name.as_str()))
                    }
                }
                LValue::Var { name, .. } => super::library::sanitize(name.as_str()),
                // Attr/Index — fall back to re-emitting rhs. Rare at
                // tail position and the double-eval risk is bounded
                // by the same caveat as Ruby's own block-tail return.
                _ => emit_expr(ctx, value),
            };
            let ret = coerce_return_value(ctx, ret);
            indent(out, depth);
            out.push_str(&format!("return {ret}\n"));
        }
        _ => {
            // Void method tails: emit the expression as a statement
            // (when it has side effects) or skip entirely for a bare
            // `nil`. Ruby's implicit-nil tail must not turn into
            // `return nil` against a Go void function.
            if ctx.void_method {
                if is_nil_lit(e) {
                    // No-op trailing nil.
                    return;
                }
                let v = emit_expr(ctx, e);
                indent(out, depth);
                out.push_str(&v);
                out.push('\n');
            } else {
                let v = emit_expr(ctx, e);
                let v = coerce_return_value(ctx, v);
                indent(out, depth);
                out.push_str(&format!("return {v}\n"));
            }
        }
    }
}

#[cfg(test)]
mod predicate_naming_tests {
    use super::{go2_method_ident, go_field_ident};

    /// Regression guard for the AR column-predicate collision in Go, which
    /// splits across two paths: method CALLS affix `?` → `_pred` (so
    /// `exists?` → `ExistsPred` never collides with a same-named method),
    /// while bare field READS strip `?` (so `deleted_at?` lands on the
    /// `DeletedAt` struct field). Both invariants are load-bearing.
    #[test]
    fn call_path_affixes_field_path_strips() {
        // Method-call path: predicate distinct from reader.
        assert_ne!(go2_method_ident("exists"), go2_method_ident("exists?"));
        assert_eq!(go2_method_ident("exists?"), "ExistsPred");
        assert_eq!(go2_method_ident("save!"), "SaveBang");
        // Field-read path: predicate resolves to the backing field.
        assert_eq!(go_field_ident("deleted_at?"), go_field_ident("deleted_at"));
        assert_eq!(go_field_ident("deleted_at?"), "DeletedAt");
    }
}
