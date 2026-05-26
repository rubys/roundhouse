//! Explicit type-coercion insertion across LibraryClass method bodies.
//!
//! Walks each Send in each method body and, where a positional arg's
//! `Ty` is narrower than the callee's declared param `Ty`, wraps the
//! arg in `ExprNode::Cast { value, target_ty }`. Downstream emitters
//! consume the Cast nodes per-target — rust2 widens
//! `HashMap<K,V>` via `into_iter().map().collect()`, go2 produces a
//! `map[string]any` conversion, TS/Crystal/Ruby treat Cast as identity
//! (their typers handle widening natively, so the Cast node is a
//! pass-through).
//!
//! This replaces emit-time back-propagation that derives the same
//! information from arg-vs-param Ty comparisons at every call site in
//! every emitter. Landing the typing intent once in the IR means each
//! emitter just consumes a uniform construct.
//!
//! Stage 2/3: Hash-widening family. When a callee's positional param
//! is declared `Hash[_, untyped]` (`Ty::Hash { value: Untyped, .. }`)
//! and the arg's inferred `Ty` is a different concrete Hash shape (or
//! the arg is an inline Hash literal), wrap the arg in `Cast`. Other
//! coercion families (`T → Option<T>` Some-wrap, `Sym → Str` key
//! rewrites) land in subsequent stages.
//!
//! Callee resolution covers three recv shapes:
//!   - `Const { path }` — class method dispatch (`ViewHelpers::render_attrs`)
//!   - `SelfRef` — sibling method in the current class
//!   - `None` (implicit self) — same as SelfRef

use crate::dialect::LibraryClass;
use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, RescueClause};
use crate::ty::{ParamKind, Ty};
use std::collections::HashMap;

/// (ClassName, method_name) → param_tys. ClassName is the last segment
/// of the LC's name (e.g. `ViewHelpers` for `ActionView::ViewHelpers`)
/// to match how `Const { path }` recv arms look up at emit time.
type CalleeRegistry = HashMap<String, HashMap<String, Vec<Ty>>>;

/// Insert `ExprNode::Cast` wrappers at call-site arg positions where
/// the callee's declared param Ty widens the arg's Ty. Mutates `lcs`
/// in place.
pub fn insert_ty_coercions(lcs: &mut [LibraryClass]) {
    let registry = build_registry(lcs);
    for lc in lcs.iter_mut() {
        let raw = lc.name.0.as_str();
        let class_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
        for method in &mut lc.methods {
            method.body = rewrite_expr(&method.body, &registry, &class_name);
        }
    }
}

/// Variant where the callee registry is built from a wider set than
/// the rewrite targets. Lets a per-batch lowering (controllers only)
/// resolve cross-class calls (Post.find from a controller body) by
/// seeing the model LCs' signatures even though they aren't being
/// rewritten in this call. Mirrors the rust2 wiring's implicit
/// behavior — there it works because all classes flow through one
/// emit pass; go2's overlay does batches.
pub fn insert_ty_coercions_with_extras(
    targets: &mut [LibraryClass],
    extras: &[LibraryClass],
) {
    let mut combined: Vec<&LibraryClass> = Vec::with_capacity(targets.len() + extras.len());
    for lc in targets.iter() {
        combined.push(lc);
    }
    for lc in extras {
        combined.push(lc);
    }
    let registry = build_registry_from(&combined);
    for lc in targets.iter_mut() {
        let raw = lc.name.0.as_str();
        let class_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
        for method in &mut lc.methods {
            method.body = rewrite_expr(&method.body, &registry, &class_name);
        }
    }
}

fn build_registry_from(lcs: &[&LibraryClass]) -> CalleeRegistry {
    let mut out: CalleeRegistry = HashMap::new();
    for lc in lcs {
        let raw = lc.name.0.as_str();
        let class_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
        let entry = out.entry(class_name).or_default();
        for m in &lc.methods {
            let param_tys: Vec<Ty> = match m.signature.as_ref() {
                Some(Ty::Fn { params, .. }) => params
                    .iter()
                    .filter(|p| {
                        !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest)
                    })
                    .map(|p| p.ty.clone())
                    .collect(),
                _ => continue,
            };
            entry.insert(m.name.as_str().to_string(), param_tys);
        }
        // AR baseline class methods. The lowerer emits them as bare
        // `<Class>_find(id int64)` / `<Class>_count()` / etc. functions
        // via `emit_ar_class_method_wrappers` rather than as MethodDefs
        // on the LC, so they don't appear in the loop above. Without
        // these entries, a controller's `Post.find(params[:id])` Send
        // never sees that `find` takes Int, and the ty_coerce_insertion
        // pass doesn't insert the Str→Int cast at the arg position.
        //
        // Detect "is an AR-chain class" via parent inheritance: the
        // model_to_library lowerer sets `parent: Some(ClassId)` for
        // every Post < ApplicationRecord shape (and recursively for
        // ApplicationRecord < ActiveRecord::Base). Concrete tables
        // (rows beyond just `id`) get the finders; abstract bases
        // skip — matches `emit_ar_class_method_wrappers`'s emit gate.
        if lc.parent.is_some() {
            // attr_reader / attr_writer methods double as the field
            // declarations the emit lifts into the struct. Class has
            // a "table" if it has attr methods beyond `id`/`id=`.
            use crate::dialect::AccessorKind;
            let has_table = lc.methods.iter().any(|m| {
                matches!(
                    m.kind,
                    AccessorKind::AttributeReader | AccessorKind::AttributeWriter
                ) && m.name.as_str().trim_end_matches('=') != "id"
            });
            if has_table {
                entry
                    .entry("find".to_string())
                    .or_insert(vec![Ty::Int]);
                entry
                    .entry("exists?".to_string())
                    .or_insert(vec![Ty::Int]);
            }
        }
    }
    out
}

fn build_registry(lcs: &[LibraryClass]) -> CalleeRegistry {
    let mut out: CalleeRegistry = HashMap::new();
    for lc in lcs {
        let raw = lc.name.0.as_str();
        let class_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
        let entry = out.entry(class_name).or_default();
        for m in &lc.methods {
            let param_tys: Vec<Ty> = match m.signature.as_ref() {
                Some(Ty::Fn { params, .. }) => params
                    .iter()
                    .filter(|p| {
                        !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest)
                    })
                    .map(|p| p.ty.clone())
                    .collect(),
                _ => continue,
            };
            entry.insert(m.name.as_str().to_string(), param_tys);
        }
    }
    out
}

/// Recursive Expr rewriter that threads the enclosing class name
/// through so SelfRef / implicit-self Sends resolve. Mirrors the shape
/// of `controller_to_library::util::map_expr` but with an extra
/// `class_name` argument that lets us look up sibling-method param Tys.
fn rewrite_expr(expr: &Expr, registry: &CalleeRegistry, class_name: &str) -> Expr {
    let new_node = match &*expr.node {
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            let new_recv = recv.as_ref().map(|r| rewrite_expr(r, registry, class_name));
            let new_args: Vec<Expr> = args
                .iter()
                .map(|a| rewrite_expr(a, registry, class_name))
                .collect();
            let new_block = block.as_ref().map(|b| rewrite_expr(b, registry, class_name));
            // Resolve the callee's class_name for this Send.
            let callee_class: Option<String> = match new_recv.as_ref().map(|r| &*r.node) {
                Some(ExprNode::Const { path }) => {
                    path.last().map(|s| s.as_str().to_string())
                }
                // SelfRef and implicit-self (recv=None) both resolve to
                // the enclosing LC.
                Some(ExprNode::SelfRef) | None => Some(class_name.to_string()),
                _ => None,
            };
            let final_args: Vec<Expr> = match callee_class
                .as_ref()
                .and_then(|c| registry.get(c))
                .and_then(|m| m.get(method.as_str()))
            {
                Some(param_tys) => new_args
                    .into_iter()
                    .enumerate()
                    .map(|(idx, arg)| {
                        let Some(param_ty) = param_tys.get(idx) else {
                            return arg;
                        };
                        if needs_hash_widening(param_ty, &arg)
                            || needs_some_wrap(param_ty, &arg)
                            || needs_value_to_primitive(param_ty, &arg)
                        {
                            wrap_in_cast(&arg, param_ty)
                        } else {
                            arg
                        }
                    })
                    .collect(),
                None => new_args,
            };
            ExprNode::Send {
                recv: new_recv,
                method: method.clone(),
                args: final_args,
                block: new_block,
                parenthesized: *parenthesized,
            }
        }
        // Recurse through every other variant that holds child Exprs.
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| rewrite_expr(e, registry, class_name)).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_expr(cond, registry, class_name),
            then_branch: rewrite_expr(then_branch, registry, class_name),
            else_branch: rewrite_expr(else_branch, registry, class_name),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: rewrite_expr(scrutinee, registry, class_name),
            arms: arms
                .iter()
                .map(|a| Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(|g| rewrite_expr(g, registry, class_name)),
                    body: rewrite_expr(&a.body, registry, class_name),
                })
                .collect(),
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite_expr(fun, registry, class_name),
            args: args.iter().map(|a| rewrite_expr(a, registry, class_name)).collect(),
            block: block.as_ref().map(|b| rewrite_expr(b, registry, class_name)),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_expr(left, registry, class_name),
            right: rewrite_expr(right, registry, class_name),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_expr(body, registry, class_name),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: rewrite_expr(recv, registry, class_name),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: rewrite_expr(recv, registry, class_name),
                    index: rewrite_expr(index, registry, class_name),
                },
                other => other.clone(),
            };
            ExprNode::Assign {
                target: new_target,
                value: rewrite_expr(value, registry, class_name),
            }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements
                .iter()
                .map(|e| rewrite_expr(e, registry, class_name))
                .collect(),
            style: *style,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| {
                    (
                        rewrite_expr(k, registry, class_name),
                        rewrite_expr(v, registry, class_name),
                    )
                })
                .collect(),
            kwargs: *kwargs,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Expr { expr } => InterpPart::Expr {
                        expr: rewrite_expr(expr, registry, class_name),
                    },
                    other => other.clone(),
                })
                .collect(),
        },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(|a| rewrite_expr(a, registry, class_name)).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise {
            value: rewrite_expr(value, registry, class_name),
        },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: rewrite_expr(expr, registry, class_name),
            fallback: rewrite_expr(fallback, registry, class_name),
        },
        ExprNode::Return { value } => ExprNode::Return {
            value: rewrite_expr(value, registry, class_name),
        },
        ExprNode::Super { args: Some(args) } => ExprNode::Super {
            args: Some(args.iter().map(|a| rewrite_expr(a, registry, class_name)).collect()),
        },
        ExprNode::Next { value: Some(v) } => ExprNode::Next {
            value: Some(rewrite_expr(v, registry, class_name)),
        },
        ExprNode::Let { name, id, value, body } => ExprNode::Let {
            name: name.clone(),
            id: *id,
            value: rewrite_expr(value, registry, class_name),
            body: rewrite_expr(body, registry, class_name),
        },
        ExprNode::MultiAssign { targets, value } => ExprNode::MultiAssign {
            targets: targets.clone(),
            value: rewrite_expr(value, registry, class_name),
        },
        ExprNode::While { cond, body, until_form } => ExprNode::While {
            cond: rewrite_expr(cond, registry, class_name),
            body: rewrite_expr(body, registry, class_name),
            until_form: *until_form,
        },
        ExprNode::Range { begin, end, exclusive } => ExprNode::Range {
            begin: begin.as_ref().map(|b| rewrite_expr(b, registry, class_name)),
            end: end.as_ref().map(|e| rewrite_expr(e, registry, class_name)),
            exclusive: *exclusive,
        },
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: rewrite_expr(body, registry, class_name),
                rescues: rescues
                    .iter()
                    .map(|r| RescueClause {
                        classes: r
                            .classes
                            .iter()
                            .map(|c| rewrite_expr(c, registry, class_name))
                            .collect(),
                        binding: r.binding.clone(),
                        body: rewrite_expr(&r.body, registry, class_name),
                    })
                    .collect(),
                else_branch: else_branch
                    .as_ref()
                    .map(|e| rewrite_expr(e, registry, class_name)),
                ensure: ensure.as_ref().map(|e| rewrite_expr(e, registry, class_name)),
                implicit: *implicit,
            }
        }
        ExprNode::Cast { value, target_ty } => ExprNode::Cast {
            value: rewrite_expr(value, registry, class_name),
            target_ty: target_ty.clone(),
        },
        // Leaves carry no children to rewrite.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
        diagnostic: expr.diagnostic.clone(),
        hint: expr.hint,
        decisions: expr.decisions,
    }
}

/// Hash-widening trigger: param is `Hash[_, untyped]` AND arg is either
/// a Hash literal OR a value whose inferred Ty is a different Hash
/// shape (concrete value-ty, not also Untyped).
fn needs_hash_widening(param_ty: &Ty, arg: &Expr) -> bool {
    let Ty::Hash { value: pv, .. } = param_ty else {
        return false;
    };
    if !matches!(pv.as_ref(), Ty::Untyped) {
        return false;
    }
    // Skip args already wrapped in Cast — idempotency.
    if matches!(&*arg.node, ExprNode::Cast { .. }) {
        return false;
    }
    if matches!(&*arg.node, ExprNode::Hash { .. }) {
        return true;
    }
    if let Some(Ty::Hash { value: av, .. }) = arg.ty.as_ref() {
        if !matches!(av.as_ref(), Ty::Untyped) {
            return true;
        }
    }
    false
}

/// Some-wrap trigger: param is `Option<U>` (`Union { Nil, U }`) and
/// arg's body-typer ty is exactly `U` (not nilable). Mirrors rust2's
/// Family 6 gates so wrapping behavior matches what Family 6 produces
/// when no Cast is present.
///
/// Two branches:
///   - Owned-producing arg (`Var`/`Send`/`Ivar`) whose `arg.ty == U`
///   - Literal `Str`/`Sym` arg with inner `U` being `Str`/`Sym`
///     (rust2 emits `Some(literal.to_string())` to widen `&'static str`
///     → owned `String`; other emitters can choose their own shape)
fn needs_some_wrap(param_ty: &Ty, arg: &Expr) -> bool {
    if matches!(&*arg.node, ExprNode::Cast { .. }) {
        return false;
    }
    let Some(inner) = peel_option(param_ty) else {
        return false;
    };
    if matches!(inner, Ty::Untyped) {
        return false;
    }
    // Owned-producing branch.
    let owned_producing = matches!(
        &*arg.node,
        ExprNode::Var { .. } | ExprNode::Send { .. } | ExprNode::Ivar { .. }
    );
    if owned_producing && arg.ty.as_ref() == Some(inner) {
        return true;
    }
    // Literal Str/Sym branch.
    if matches!(
        &*arg.node,
        ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
    ) && matches!(inner, Ty::Str | Ty::Sym)
    {
        return true;
    }
    false
}

/// Value-to-primitive narrowing trigger: param is a primitive
/// (`Str`/`Sym`/`Int`/`Float`/`Bool`) AND arg's body-typer ty contains
/// `Untyped` (directly or inside a Union). This is the "wide runtime
/// value flowing into a typed slot" pattern — boxed value gets unboxed
/// at the use site.
///
/// Cross-target value: every strict-typed target has this shape
///   - Rust: serde_json::Value → String / i64
///   - Go: interface{} → string / int64
///   - Spinel: sp_RbVal → const char * / int
///   - Ruby (for spinel typer benefit): poly value → narrowed type via to_s/to_i
///
/// rust2 consumes via its existing Cast arm (`coerce_arg_for_field_ty`
/// has the Value→primitive narrowing); ruby consumes via Stage 5's
/// emit_cast; go2 consumes via emit_cast's primitive arm.
fn needs_value_to_primitive(param_ty: &Ty, arg: &Expr) -> bool {
    if matches!(&*arg.node, ExprNode::Cast { .. }) {
        return false;
    }
    if !matches!(
        param_ty,
        Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool
    ) {
        return false;
    }
    let Some(arg_ty) = arg.ty.as_ref() else {
        return false;
    };
    // If arg is already the same primitive type, no narrowing needed.
    if arg_ty == param_ty {
        return false;
    }
    // Untyped (interface{}/serde_json::Value/etc) — wide runtime
    // value flowing into a typed slot.
    if ty_contains_untyped(arg_ty) {
        return true;
    }
    // Nullable-primitive flowing into a non-nullable typed slot —
    // `params[:id]` typed Union<Str, Nil> reaching `Post.find(id: Int)`.
    // Without coercion the call lands with a string-or-nil where the
    // target wants Int. Insert a Cast so the emit handles the parse
    // + zero-fallback (Ruby's `nil.to_i == 0` semantics) at the call
    // site. Fires for any "Union with a Nil arm and a primitive arm,
    // target primitive differs from the arm" shape.
    if let Ty::Union { variants } = arg_ty {
        let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
        let has_other_primitive = variants.iter().any(|v| {
            matches!(v, Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool) && v != param_ty
        });
        if has_nil && has_other_primitive {
            return true;
        }
    }
    false
}

/// True when `ty` contains a `Ty::Untyped` anywhere — directly or
/// inside a `Union` variant. Duplicates `emit::rust2::expr::util::
/// ty_contains_untyped` because the lowerer is target-neutral.
fn ty_contains_untyped(ty: &Ty) -> bool {
    match ty {
        Ty::Untyped => true,
        Ty::Union { variants } => variants.iter().any(ty_contains_untyped),
        _ => false,
    }
}

/// Peel `Option<T>` (`Union { Nil, T }`) to its inner `T`. Returns
/// `None` when the param isn't a 2-variant Union with one Nil arm.
/// Mirrors rust2's `is_option_ty` + `peel_nil` shape but as a single
/// idiomatic helper for this lowerer.
fn peel_option(ty: &Ty) -> Option<&Ty> {
    let Ty::Union { variants } = ty else {
        return None;
    };
    if variants.len() != 2 {
        return None;
    }
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    if !has_nil {
        return None;
    }
    variants.iter().find(|v| !matches!(v, Ty::Nil))
}

fn wrap_in_cast(arg: &Expr, target_ty: &Ty) -> Expr {
    Expr {
        span: arg.span,
        node: Box::new(ExprNode::Cast {
            value: arg.clone(),
            target_ty: target_ty.clone(),
        }),
        ty: Some(target_ty.clone()),
        effects: arg.effects.clone(),
        leading_blank_line: arg.leading_blank_line,
        diagnostic: None,
        hint: None,
        decisions: 0,
    }
}
