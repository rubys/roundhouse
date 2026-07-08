//! Go-specific IR→IR lowerings.
//!
//! These passes run after `src/lower/`'s target-agnostic lowerings
//! and BEFORE go2's emit walk. They rewrite IR shapes that translate
//! poorly to Go into shapes that go2's emit can produce idiomatic Go
//! from.
//!
//! ## Why Go specifically
//!
//! Go is the only currently-emitted target without a native nilable
//! scalar type or `Option`-like wrapper. The framework Ruby uses
//! nil-friendly idioms freely (`v = m[k]; if !v.nil?`,
//! `arr.first&.thing`, `ENV["FOO"] || "default"`) that translate
//! directly to Crystal (`String?`), Rust (`Option<T>`), TypeScript
//! (`T | undefined`), and Ruby itself. Go's `map[K]V` returns the
//! zero value for missing keys with no nilable wrapper, so the same
//! shapes produce `vet` errors at the nil comparison.
//!
//! Rather than build emit-time peephole logic that has to recognize
//! these patterns across statement boundaries (awkward in a bottom-
//! up emit walk), each Go-incompatible Ruby idiom gets a dedicated
//! IR→IR lowerer here. Each pass is a pure function: `Vec<LibraryClass>`
//! in, `Vec<LibraryClass>` out. Composes cleanly with the others;
//! testable in isolation via assertions on the transformed IR.
//!
//! ## Pass order
//!
//! Passes are listed in `lower_for_go` below in execution order.
//! Most should be commutative (operate on disjoint IR shapes), but
//! the orchestrator runs them sequentially so order-dependent rewrites
//! stay deterministic. When two passes overlap, document the
//! interaction at the call site.
//!
//! ## Relationship to rust2's approach
//!
//! rust2 didn't need this layer — Rust's `Option<T>` and `Result<T, E>`
//! gave each nil-prone shape a natural target form, with `str_color`
//! (an analyzer pass, not a lowerer) handling the one remaining
//! Rust-specific concern (String/&str ownership coercion). Go has no
//! analog; the rewrites have to happen IR-level. Future Go-like
//! targets (Kotlin/Swift with their own optional discipline) could
//! reuse these passes wholesale or as a starting point.

use crate::dialect::LibraryClass;

/// Apply each Go-specific lowering pass in order. Called from
/// `emit::go2::emit_overlay_files` via the `go_units` transform
/// hook — every transpiled framework class flows through this
/// pipeline before go2/emit sees it.
pub fn lower_for_go(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
    lower_for_go_with_extras(classes, &[])
}

/// Variant that threads a wider callee registry through ty-coerce
/// insertion. Used when lowering a batch of controllers that need
/// to resolve cross-class calls into models (e.g. `Post.find(...)`
/// from a controller body needs to see Post's signature to decide
/// whether to wrap the arg in a Cast for the Int param). Without
/// `extras` the per-batch registry only sees the batch's own
/// classes, and cross-batch coercion opportunities go unrealized.
pub fn lower_for_go_with_extras(
    classes: Vec<LibraryClass>,
    extras: &[LibraryClass],
) -> Vec<LibraryClass> {
    let classes = nil_check_to_comma_ok::apply(classes);
    let mut classes = nil_to_zero_for_string_fields::apply(classes);
    let extras_refs: Vec<&LibraryClass> = extras.iter().collect();
    crate::lower::insert_ty_coercions_with_extras(&mut classes, &extras_refs);
    classes
}

/// Pattern: `v = m[k]; if !v.nil? { body using v }`.
///
/// Ruby's `Hash#[]` returns nil for missing keys, then the subsequent
/// `.nil?` guard filters. In Go, `m[k]` on `map[K]V` returns the
/// zero value of `V` for missing keys; the nilness information is
/// erased. The comma-ok form `v, ok := m[k]; if ok` is Go's native
/// equivalent of Ruby's nil check, but the rewrite has to span the
/// assignment and the conditional — too coarse for the per-Send
/// emit_expr walk.
///
/// This pass walks each method body's `Seq` looking for the
/// two-statement pattern and rewrites to a synthesized Send with
/// method name `_go_try_fetch` and the original then-branch as a
/// block. The emit side (`expr::emit_send`) recognizes the magic
/// method name and produces:
///
/// ```text
/// func() {
///     if v, ok := m[k]; ok {
///         <body>
///     }
/// }()
/// ```
///
/// The IIFE isolates the comma-ok scope so subsequent uses of `v`
/// (if any) reference the OUTER scope, not the inner.
pub mod nil_check_to_comma_ok {
    use crate::dialect::LibraryClass;
    use crate::expr::{Expr, ExprNode, LValue, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;
    use crate::ty::Ty;

    /// Magic method name on the synthesized Send. Emit recognizes
    /// this and produces comma-ok Go.
    pub const SENTINEL_METHOD: &str = "_go_try_fetch";

    pub fn apply(mut classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
        for class in classes.iter_mut() {
            for method in class.methods.iter_mut() {
                method.body = transform(&method.body);
            }
        }
        classes
    }

    /// Bottom-up traversal: children first, then look for the pair
    /// pattern at the current level (only meaningful inside Seq).
    fn transform(e: &Expr) -> Expr {
        match &*e.node {
            ExprNode::Seq { exprs } => {
                let transformed: Vec<Expr> = exprs.iter().map(transform).collect();
                let rewritten = scan_pairs(&transformed);
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Seq { exprs: rewritten });
                new_e
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::If {
                    cond: transform(cond),
                    then_branch: transform(then_branch),
                    else_branch: transform(else_branch),
                });
                new_e
            }
            ExprNode::Lambda { params, block_param, body, block_style } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Lambda {
                    params: params.clone(),
                    block_param: block_param.clone(),
                    body: transform(body),
                    block_style: *block_style,
                });
                new_e
            }
            ExprNode::Send { recv, method, args, block, parenthesized } => {
                let new_recv = recv.as_ref().map(transform);
                let new_args: Vec<Expr> = args.iter().map(transform).collect();
                let new_block = block.as_ref().map(transform);
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Send {
                    recv: new_recv,
                    method: method.clone(),
                    args: new_args,
                    block: new_block,
                    parenthesized: *parenthesized,
                });
                new_e
            }
            ExprNode::Assign { target, value } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Assign {
                    target: target.clone(),
                    value: transform(value),
                });
                new_e
            }
            ExprNode::Return { value } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Return {
                    value: transform(value),
                });
                new_e
            }
            _ => e.clone(),
        }
    }

    /// Walk Seq pairwise, replacing matched `Assign+If` pairs with
    /// the synthesized Send. Single passes (no recursive re-scan)
    /// — the pattern doesn't nest in itself.
    fn scan_pairs(exprs: &[Expr]) -> Vec<Expr> {
        let mut out = Vec::with_capacity(exprs.len());
        let mut i = 0;
        while i < exprs.len() {
            if i + 1 < exprs.len() {
                if let Some(synth) = try_rewrite_pair(&exprs[i], &exprs[i + 1]) {
                    out.push(synth);
                    i += 2;
                    continue;
                }
            }
            out.push(exprs[i].clone());
            i += 1;
        }
        out
    }

    /// Match `Assign { Var(v), Send(m, "[]", [k]) } + If { !Var(v).nil?, then, Nil }`.
    fn try_rewrite_pair(a: &Expr, b: &Expr) -> Option<Expr> {
        let (var_name, map_expr, key_expr) = match &*a.node {
            ExprNode::Assign {
                target: LValue::Var { name, .. },
                value,
            } => match &*value.node {
                ExprNode::Send {
                    recv: Some(map),
                    method,
                    args,
                    block: None,
                    ..
                } if method.as_str() == "[]"
                    && args.len() == 1
                    && receiver_is_hash(map) =>
                {
                    (name.clone(), map.clone(), args[0].clone())
                }
                _ => return None,
            },
            _ => return None,
        };

        let (cond, then_branch) = match &*b.node {
            ExprNode::If {
                cond,
                then_branch,
                else_branch,
            } if is_nil_lit(else_branch) => (cond, then_branch.clone()),
            _ => return None,
        };

        if !is_not_nil_check_on_var(cond, var_name.as_str()) {
            return None;
        }

        let lambda = Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                params: vec![var_name.clone()],
                block_param: None,
                body: then_branch,
                block_style: Default::default(),
            },
        );
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(map_expr),
                method: Symbol::from(SENTINEL_METHOD),
                args: vec![key_expr],
                block: Some(lambda),
                parenthesized: true,
            },
        ))
    }

    fn is_nil_lit(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
    }

    /// True when the analyzer-set Ty on the receiver indicates a
    /// Hash (possibly nullable). Restricts the rewrite to actual map
    /// values; without this, `self[k]` on a `*ActionDispatchFlash`
    /// (where `[]` is a method call on the struct, not a map index)
    /// would falsely match. Class-typed receivers route their `[]`
    /// through go2/emit's existing OpGet dispatch instead.
    fn receiver_is_hash(recv: &Expr) -> bool {
        match recv.ty.as_ref() {
            Some(Ty::Hash { .. }) => true,
            Some(Ty::Union { variants }) => variants.iter().any(|v| matches!(v, Ty::Hash { .. })),
            _ => false,
        }
    }

    /// True if `cond` matches `!Var(var_name).nil?`. Handles the two
    /// IR shapes documented in `src/analyze/body/narrowing.rs`:
    /// `Send { recv: Some(Send{Var(v), "nil?"}), method: "!" }` (the
    /// dominant shape) and `Send { recv: None, method: "!",
    /// args: [Send{Var(v), "nil?"}] }`.
    fn is_not_nil_check_on_var(cond: &Expr, var_name: &str) -> bool {
        let inner = match &*cond.node {
            ExprNode::Send {
                recv: Some(r),
                method,
                args,
                ..
            } if method.as_str() == "!" && args.is_empty() => r,
            ExprNode::Send {
                recv: None,
                method,
                args,
                ..
            } if method.as_str() == "!" && args.len() == 1 => &args[0],
            _ => return false,
        };
        let ExprNode::Send {
            recv: Some(v_expr),
            method: nil_method,
            args: nil_args,
            ..
        } = &*inner.node
        else {
            return false;
        };
        if nil_method.as_str() != "nil?" || !nil_args.is_empty() {
            return false;
        }
        matches!(&*v_expr.node, ExprNode::Var { name, .. } if name.as_str() == var_name)
    }
}

/// Pattern: `@field = nil` where the field's declared Ty is
/// `Union[Str, Nil]` (RBS `String?`).
///
/// With `go_ty_stub` now mapping `Union[Str, Nil]` to Go `string`
/// (the empty-as-nil convention), an emitted `self.Field = nil`
/// fails to typecheck — `string` doesn't accept nil. Rewrite the
/// assignment value to the empty-string literal so the Go shape
/// stays valid. Reads of the field aren't touched: a bare `@field`
/// already emits as `self.Field` (a string read), and `.nil?`
/// against the value is handled by the emit-time peephole (which
/// checks the recv's Union[Str, Nil] Ty and emits `recv == ""`).
///
/// Only the LValue::Ivar { name } and `LValue::Attr { recv: Self,
/// name }` shapes match — Var (local) assigns to nil aren't field
/// writes; their target type is whatever the local was declared
/// with, not the class's field Ty.
pub mod nil_to_zero_for_string_fields {
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
    use crate::expr::{Expr, ExprNode, LValue, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;
    use crate::ty::Ty;
    use std::collections::HashMap;

    pub fn apply(mut classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
        for class in classes.iter_mut() {
            let fields = collect_string_nullable_fields(&class.methods);
            if fields.is_empty() {
                continue;
            }
            for method in class.methods.iter_mut() {
                method.body = transform(&method.body, &fields);
            }
        }
        classes
    }

    /// Collect ivar names whose declared Ty is `Union[Str, Nil]`
    /// (or `Union[Sym, Nil]`), derived from attr_reader/writer
    /// signatures. These are exactly the fields that go_ty_stub now
    /// emits as Go `string` — and where `self.Field = nil` would
    /// fail to typecheck.
    fn collect_string_nullable_fields(methods: &[MethodDef]) -> HashMap<String, ()> {
        let mut out = HashMap::new();
        for m in methods {
            if !matches!(m.receiver, MethodReceiver::Instance) {
                continue;
            }
            let ty = match m.kind {
                AccessorKind::AttributeReader => match m.signature.as_ref() {
                    Some(Ty::Fn { ret, .. }) => (**ret).clone(),
                    _ => continue,
                },
                AccessorKind::AttributeWriter => match m.signature.as_ref() {
                    Some(Ty::Fn { params, .. }) => match params.first() {
                        Some(p) => p.ty.clone(),
                        None => continue,
                    },
                    _ => continue,
                },
                _ => continue,
            };
            if is_nullable_string(&ty) {
                let name = m.name.as_str().trim_end_matches('=').to_string();
                out.insert(name, ());
            }
        }
        out
    }

    fn is_nullable_string(ty: &Ty) -> bool {
        let Ty::Union { variants } = ty else { return false };
        let non_nil: Vec<&Ty> = variants
            .iter()
            .filter(|t| !matches!(t, Ty::Nil))
            .collect();
        matches!(non_nil.as_slice(), [Ty::Str] | [Ty::Sym])
    }

    fn transform(e: &Expr, fields: &HashMap<String, ()>) -> Expr {
        match &*e.node {
            ExprNode::Assign { target, value }
                if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil })
                    && is_string_field_target(target, fields) =>
            {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Assign {
                    target: target.clone(),
                    value: empty_string_expr(),
                });
                new_e
            }
            ExprNode::Seq { exprs } => {
                let new_exprs: Vec<Expr> = exprs.iter().map(|s| transform(s, fields)).collect();
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Seq { exprs: new_exprs });
                new_e
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::If {
                    cond: transform(cond, fields),
                    then_branch: transform(then_branch, fields),
                    else_branch: transform(else_branch, fields),
                });
                new_e
            }
            ExprNode::Lambda { params, block_param, body, block_style } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Lambda {
                    params: params.clone(),
                    block_param: block_param.clone(),
                    body: transform(body, fields),
                    block_style: *block_style,
                });
                new_e
            }
            ExprNode::Send { recv, method, args, block, parenthesized } => {
                let new_recv = recv.as_ref().map(|r| transform(r, fields));
                let new_args: Vec<Expr> = args.iter().map(|a| transform(a, fields)).collect();
                let new_block = block.as_ref().map(|b| transform(b, fields));
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Send {
                    recv: new_recv,
                    method: method.clone(),
                    args: new_args,
                    block: new_block,
                    parenthesized: *parenthesized,
                });
                new_e
            }
            ExprNode::Assign { target, value } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Assign {
                    target: target.clone(),
                    value: transform(value, fields),
                });
                new_e
            }
            ExprNode::Return { value } => {
                let mut new_e = e.clone();
                new_e.node = Box::new(ExprNode::Return {
                    value: transform(value, fields),
                });
                new_e
            }
            _ => e.clone(),
        }
    }

    fn is_string_field_target(target: &LValue, fields: &HashMap<String, ()>) -> bool {
        match target {
            LValue::Ivar { name } => fields.contains_key(name.as_str()),
            LValue::Attr { recv, name } => {
                matches!(&*recv.node, ExprNode::SelfRef) && fields.contains_key(name.as_str())
            }
            _ => false,
        }
    }

    fn empty_string_expr() -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str { value: String::new() },
            },
        )
    }

    // Re-exported by parent for tests / pipeline.
     const _: fn(&Symbol) = |_| ();
}
