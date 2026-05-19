//! Receiver-aware method bridges. The single `try_recv_typed_method`
//! function probes the `(recv, method, args)` tuple for any of ~15
//! Ruby-method-on-typed-receiver shapes (`[]`, `[]=`, `is_a?`, `to_h`,
//! `merge`, `fetch`, `tr`, `split`, `gsub`, `length`, `capitalize`,
//! `nil?`, `self.class.X`, plus the generic recv-Ty bridge from
//! `dispatch.rs`). Returns `Some(emit)` on a match; `None` to fall
//! through to the method-name-bridge path in `mod.rs`.

use crate::expr::{Expr, ExprNode, Literal};

use super::super::util::{
    coerce_to_value, is_builtin_container_class, is_copy_ty, peel_nil,
    rewrite_method_name,
};
use super::super::literal::emit_is_a;
use super::super::{
    emit_expr, in_module_singleton, ivar_field_ty,
    module_singleton_slot_name, recv_var_back_propagated_hash_kv,
};
use super::dispatch::dispatch_method_by_recv_ty;

pub(super) fn try_recv_typed_method(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    let r = recv?;
        if method == "[]" && args.len() == 1 {
            // Peel `Union<T, Nil>` from the recv Ty so receivers bound
            // via `let x = arr[i]` (typed `T | Nil` by the body-typer's
            // Ruby-semantics view) match the same branches as the
            // plain receiver case. Emit chose panic-on-miss for `[]`,
            // so the runtime value really is T.
            let recv_ty = r.ty.as_ref().map(peel_nil);
            let arg_ty = args[0].ty.as_ref().map(peel_nil);
            // Range index on Str/Vec receiver — `pp[1..]`. The Range
            // node emits its endpoints unmodified (`1_i64..`), but
            // slice indexing needs `usize`. Wrap the rendered range
            // in a re-cast so `&pp[(1) as usize..]` compiles.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
                    | Some(crate::ty::Ty::Array { .. })
            ) {
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    let b = begin
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let e = end
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let op = if *exclusive { ".." } else { "..=" };
                    let range_s = match (begin.is_some(), end.is_some()) {
                        (true, true) => format!("{b}{op}{e}"),
                        (true, false) => format!("{b}.."),
                        (false, true) => format!("..{e}"),
                        (false, false) => "..".to_string(),
                    };
                    // Str slices need a `&` prefix to yield `&str`; Vec
                    // slices likewise yield `&[T]`. Either way the
                    // caller treats it as borrowed.
                    return Some(format!("&{}[{range_s}]", emit_expr(r)));
                }
            }
            // Negative-int literal index on Vec/Str (`arr[-1]` = last
            // element, `arr[-2]` = second to last). Rust's `Index`
            // panics on negative-cast-to-usize (`(-1_i64) as usize`
            // is a huge number). Rewrite to `recv[recv.len() - N]`
            // where N is the absolute negative. Mirrors the TS emit
            // (`recv[recv.length-N]`) at line ~1600 of typescript/expr.rs.
            // Only fires for literal negatives — dynamic negative
            // indices would need a runtime branch, which the framework
            // patterns we ship today don't require.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            ) {
                if let ExprNode::Lit { value: Literal::Int { value } } = &*args[0].node {
                    if *value < 0 {
                        let recv_s = emit_expr(r);
                        let abs = -value;
                        // Vec<T>::Index returns `&T`; clone to produce
                        // the owned `T` Ruby's `arr[-1]` semantics
                        // delivers. Callers (e.g. `Base.last`'s tail
                        // wrapped in `Some(...)`) need owned T to
                        // match the `Option<T>` return type.
                        if matches!(recv_ty, Some(crate::ty::Ty::Array { .. })) {
                            return Some(format!(
                                "{recv_s}[{recv_s}.len() - {abs}_usize].clone()"
                            ));
                        }
                        return Some(format!("{recv_s}[{recv_s}.len() - {abs}_usize]"));
                    }
                }
            }
            // Slice/Vec indexing needs `usize`. Ruby integers (including
            // numeric loop counters like `let mut i = 0_i64`) lower to
            // `i64`; without a cast the `Index<i64>` impl is missing
            // and rustc rejects with E0277. Recv-type-aware: only
            // fires when recv is Ty::Array and the index expression's
            // Ty is Ty::Int. HashMap indexing keeps the bare form
            // (HashMap<K, V> indexes by &K, the user-supplied key
            // is already the right type).
            if let Some(crate::ty::Ty::Array { elem }) = recv_ty {
                if matches!(arg_ty, Some(crate::ty::Ty::Int)) {
                    // `Vec<T>::Index` returns `&T`; passing the result
                    // to a function taking `T` by value (the typical
                    // Ruby-emit consuming-arg shape) requires
                    // materializing an owned T. Append `.clone()` for
                    // non-Copy element types — mirrors the negative-
                    // index branch's clone (the same E0507 motivates
                    // both). Copy elems (i64, f64, bool) need no
                    // suffix.
                    let suffix = if is_copy_ty(elem) { "" } else { ".clone()" };
                    return Some(format!(
                        "{}[({}) as usize]{}",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        suffix
                    ));
                }
            }
            // Ty::Class recv routing. Flash / Session expose `[]` via
            // a `.get(k)` method (the framework runtime's HWIA-shape
            // shim) rather than `Index`; emit accordingly. Non-builtin
            // Ty::Class recv routes through the `get_index` method
            // (the `sanitize_ident`'s `[]` → `get_index` rewrite).
            // Builtin containers (Hash, Array) keep the bracket form.
            if let Some(crate::ty::Ty::Class { id, .. }) = recv_ty {
                let cls = id.0.as_str();
                let cls_leaf = cls.rsplit("::").next().unwrap_or(cls);
                if matches!(cls_leaf, "Flash" | "Session") {
                    return Some(format!(
                        "{}.get({})",
                        emit_expr(r),
                        emit_expr(&args[0])
                    ));
                }
                if !is_builtin_container_class(cls) {
                    return Some(format!(
                        "{}.get_index({})",
                        emit_expr(r),
                        emit_expr(&args[0])
                    ));
                }
            }
            return Some(format!("{}[{}]", emit_expr(r), emit_expr(&args[0])));
        }
        // Ruby `String#[](start, length)` — byte-slice with start +
        // length. Ruby's substring semantics are owned (a fresh
        // `String` each call), so the emit produces owned `String`
        // via `.to_string()` on the slice. Without ownership, a
        // pattern like `ms = padded[0, 3]` reassigns an outer-scope
        // binding to a slice of an inner-scope local (`padded` drops
        // at end of the if-block), tripping E0597. Producing String
        // at the slice site matches Ruby semantics + lets the
        // multi-assign coordination in `analyze::str_color` line up
        // the binding's declared type (`let mut ms: String = …`)
        // with subsequent slice assignments. Args land here as `i64`
        // from the body-typer, hence the `as usize` casts. Caveat:
        // `start_s` is duplicated in the emitted source; fine for
        // the literal/local arg shapes seen in practice (`str[0,
        // 10]`, `str[0, cutoff]`).
        if method == "[]" && args.len() == 2 {
            let recv_s = emit_expr(r);
            let start_s = emit_expr(&args[0]);
            let len_s = emit_expr(&args[1]);
            return Some(format!(
                "(&{recv_s}[({start_s}) as usize..(({start_s}) + ({len_s})) as usize]).to_string()"
            ));
        }
        if method == "[]=" && args.len() == 2 {
            // Module-singleton Ivar `[]=`: `@slots[k] = v` in a
            // `def self.foo` body needs to mutate the static
            // `Mutex<Option<HashMap>>` slot through
            // `get_or_insert_with`. The default Ivar-read emit
            // returns a cloned snapshot; bracket-writing into that
            // clone is a silent runtime bug AND fails the surface
            // type-check (`HashMap<String, String>` value type vs
            // `&str` arg, which is what `view_helpers.rs:76/92`
            // reported). Key/value get `.to_string()` appended —
            // the body-typer types `@slots` from the `{}` init as
            // `Hash<Untyped, Untyped>`, so the str_color Hash-K/V
            // coloring doesn't fire here; an unconditional append
            // is idempotent on already-String shapes.
            if in_module_singleton() {
                if let ExprNode::Ivar { name } = &*r.node {
                    let slot = module_singleton_slot_name(name.as_str());
                    let k = emit_expr(&args[0]);
                    let v = emit_expr(&args[1]);
                    return Some(format!(
                        "{{ {slot}.lock().unwrap().get_or_insert_with(std::collections::HashMap::new).insert(({k}).to_string(), ({v}).to_string()); }}"
                    ));
                }
            }
            // Mirror the LValue::Index Flash/Session bridge — when
            // the Send-shape `recv.[]=(k, v)` lands on a Flash or
            // Session typed receiver, route through the hand-written
            // `.set(key, value)` method (no IndexMut impl). Per-app
            // model classes implement column-specific `[]=` overrides
            // via the IR's regular method path, not this branch.
            //
            // Type detection covers two channels: `recv.ty` (when the
            // body-typer set it) and an `Ivar { name }` recv whose
            // declared field type sits in IVAR_TYPES.
            let recv_class = match r.ty.as_ref() {
                Some(crate::ty::Ty::Class { id, .. }) => Some(id.0.as_str().to_string()),
                _ => match &*r.node {
                    ExprNode::Ivar { name } => match ivar_field_ty(name.as_str()) {
                        Some(crate::ty::Ty::Class { id, .. }) => {
                            Some(id.0.as_str().to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                },
            };
            if let Some(cls) = recv_class.as_deref() {
                if matches!(cls, "Flash" | "ActionDispatch::Flash") {
                    // Flash::set takes Option<String>; wrap non-Option
                    // rhs in Some(...). See the LValue::Index Flash
                    // branch above — same shape, just the Send-form
                    // entry point (`@flash.[]=(k, v)` in the lowered IR).
                    let rhs = emit_expr(&args[1]);
                    let rhs_is_option = matches!(
                        args[1].ty.as_ref(),
                        Some(crate::ty::Ty::Union { variants })
                            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                    );
                    let wrapped = if rhs_is_option {
                        rhs
                    } else {
                        format!("Some({rhs})")
                    };
                    return Some(format!(
                        "{}.set({}, {wrapped})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    ));
                }
                if matches!(cls, "Session" | "ActionDispatch::Session") {
                    return Some(format!(
                        "{}.set({}, {})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        emit_expr(&args[1]),
                    ));
                }
                // Non-builtin Ty::Class — route through `set_index`
                // (the `[]=` operator-method rewrite). Wrap value RHS
                // with Value::from when its Ty isn't already
                // serde_json::Value-shaped.
                if !is_builtin_container_class(cls) {
                    let rhs = emit_expr(&args[1]);
                    let coerced_rhs = coerce_to_value(&args[1], &rhs);
                    return Some(format!(
                        "{}.set_index({}, {coerced_rhs})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    ));
                }
            }
            // Ty::Hash recv via Send-`[]=` — HashMap doesn't implement
            // IndexMut, so `recv[k] = v` syntax fails (E0594). Mirror
            // the LValue::Index Ty::Hash branch and route through
            // `.insert(k, v)`. Wrap in `{ ...; }` for the no-else
            // `if cond { ... }` context (HashMap::insert returns
            // Option<V>; trailing `;` makes the block `()` typed).
            //
            // K/V coercion via `local_var_ty`: when the recv is a Var
            // whose locally-tracked type (e.g. back-propagated from
            // the function's return signature, see
            // `empty_hash_return_ty`) is `Hash<Str|Sym, Str|Sym>`, the
            // body-typer's IR-level `r.ty` may still be the
            // `Hash<Untyped, Untyped>` shape from the `{}` init — so
            // str_color's Hash-recv coloring doesn't fire and the args
            // arrive un-coerced. Append `.to_string()` per arg whose
            // K/V slot is String-shaped. Idempotent for already-String.
            let recv_hash_kv = recv_var_back_propagated_hash_kv(r);
            let is_hash_recv =
                matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
                    || recv_hash_kv.is_some();
            if is_hash_recv {
                let coerce_str = |slot: Option<&crate::ty::Ty>, raw: String| -> String {
                    match slot {
                        Some(crate::ty::Ty::Str | crate::ty::Ty::Sym) => {
                            format!("({raw}).to_string()")
                        }
                        _ => raw,
                    }
                };
                let k = emit_expr(&args[0]);
                let v = emit_expr(&args[1]);
                let (kk, vv) = match &recv_hash_kv {
                    Some((k_ty, v_ty)) => (
                        coerce_str(Some(k_ty), k),
                        coerce_str(Some(v_ty), v),
                    ),
                    None => (k, v),
                };
                return Some(format!("{{ {}.insert({kk}, {vv}); }}", emit_expr(r)));
            }
            return Some(format!("{}[{}] = {}", emit_expr(r), emit_expr(&args[0]), emit_expr(&args[1])));
        }
        // Ruby `value.is_a?(Class)` runtime type check. Rust has no
        // generic analog — every type is statically known. For the
        // `serde_json::Value`-typed gradual-escape recv (the common
        // shape after Ty::Untyped commits to `serde_json::Value`),
        // map the known Ruby class names to serde_json predicates;
        // user-defined classes degrade to `false` with a comment
        // (always-false branch in a chain like normalize_value, the
        // next branch handles the real case).
        if method == "is_a?" && args.len() == 1 {
            return Some(emit_is_a(r, &args[0]));
        }
        // `hash.to_h` — Ruby identity on Hash (returns self). Crystal
        // uses it to bridge NamedTuple → Hash; Rust has no NamedTuple,
        // so on a HashMap-typed recv the method has no analog and
        // `.clone()` preserves the "fresh owned hash" semantics. The
        // Flash/Session typed structs have their own `to_h()` method
        // returning HashMap<String, String>; those go through the
        // generic dispatch path below (recv ty is not Ty::Hash there).
        // Recv typed Untyped/None falls through too — call sites that
        // need the .to_h() on a serde_json::Value will rely on
        // separate emit work for the Value method-call fix.
        if method == "to_h"
            && args.is_empty()
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return Some(format!("{}.clone()", emit_expr(r)));
        }
        // `hash.merge(other)` — Ruby Hash#merge returns a new Hash
        // with `other`'s entries layered on top. Rust HashMap has no
        // built-in merge AND the typical call site has mixed K/V
        // types (literal `(&str, &str)` merged with parameter
        // `HashMap<String, Value>`), which a generic trait can't
        // bridge. `merge_attrs` from runtime/rust/hash_ext.rs takes
        // both sides as IntoIterator over (K: Into<String>, V:
        // Into<Value>) and produces a unified
        // `HashMap<String, Value>` — matches what `render_attrs`,
        // `r#where`, and similar consumers expect. Recv-types
        // outside Ty::Hash (Flash, Session) keep their own `merge`
        // method via the generic dispatch path below.
        if method == "merge"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return Some(format!(
                "merge_attrs({}, {})",
                emit_expr(r),
                emit_expr(&args[0]),
            ));
        }
        // `hash.fetch(key, default)` — Ruby Hash#fetch returns the
        // value at key or `default` when missing. Rust HashMap has
        // `.get(K) -> Option<&V>`; bridge via `.cloned()` (nil
        // default; `Option::None` unifies trivially) or
        // `.cloned().unwrap_or(default)` otherwise. Recv must be a
        // Ty::Hash that's a real value at the call site.
        // Const receivers (e.g. `STATUS_CODES.fetch(...)`) are
        // accepted now that module-level Hash constants emit as
        // `static NAME: LazyLock<HashMap<...>>` — the LazyLock
        // auto-deref makes `STATUS_CODES.get(&k)` resolve through to
        // the inner HashMap.
        if method == "fetch"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let key_s = emit_expr(&args[0]);
            let default_is_nil = matches!(
                &*args[1].node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if default_is_nil {
                return Some(format!("{recv_s}.get({key_s}).cloned()"));
            }
            // Coerce the default against the receiver's value Ty so a
            // `{}` Hash literal default against `HashMap<String, Value>`
            // renders as `Value::Object(...)`, not bare `HashMap::new()`
            // (which trips E0308 at the unwrap_or slot).
            let default_s = if let Some(crate::ty::Ty::Hash { value: rv, .. }) =
                r.ty.as_ref().map(peel_nil)
            {
                super::coerce::coerce_arg_for_param_ty(&args[1], &rv)
            } else {
                emit_expr(&args[1])
            };
            return Some(format!("{recv_s}.get({key_s}).cloned().unwrap_or({default_s})"));
        }
        // `s.tr(from, to)` — character translation. Limited to
        // single-char from/to (the framework Ruby use case is
        // `inner_k.to_s.tr("_", "-")` in render_attrs). Multi-char
        // and ranges fall through to generic dispatch so the gap
        // surfaces as a compile error rather than silently mis-
        // emitting. Mirrors the TypeScript emit's `tr` peephole.
        if method == "tr" && args.len() == 2 {
            if let (
                ExprNode::Lit { value: Literal::Str { value: from } },
                ExprNode::Lit { value: Literal::Str { value: to } },
            ) = (&*args[0].node, &*args[1].node)
            {
                if from.chars().count() == 1 && to.chars().count() == 1 {
                    let recv_s = emit_expr(r);
                    return Some(format!("{recv_s}.replace({from:?}, {to:?})"));
                }
            }
        }
        // `str.split(sep)` — Ruby returns an Array; Rust returns a
        // lazy `Split` iterator that doesn't support `.len()` or
        // indexing. Eagerly collect to Vec<&str> so downstream
        // `parts.length` / `parts[i]` (the typical router-table
        // walking pattern) compiles. Recv-type-aware: only fires on
        // Ty::Str receivers; user-defined `split` on other types
        // stays intact.
        if method == "split"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let sep_s = emit_expr(&args[0]);
            return Some(format!("{recv_s}.split({sep_s}).collect::<Vec<&str>>()"));
        }
        // `s.gsub(pattern, table)` with a `Ty::Class { Regexp }` first
        // arg and a `Ty::Hash` second arg — the canonical Ruby idiom
        // for table-driven character replacement (used by
        // `view_helpers.html_escape` and `json_builder.encode_string`).
        // Rust analog: `pattern.replace_all(&s, |caps| table[&caps[0]])`
        // returning the owned String. The pattern is typically a
        // module-level `LazyLock<Regex>` constant, the table a
        // `LazyLock<HashMap<&'static str, &'static str>>` — both auto-
        // deref to their inner types at the call site.
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(
                args[0].ty.as_ref().map(peel_nil),
                Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Regexp"
            )
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let pat_s = emit_expr(&args[0]);
            let table_s = emit_expr(&args[1]);
            return Some(format!(
                "{pat_s}.replace_all(&{recv_s}, |__caps: &regex::Captures| -> String \
                 {{ (*{table_s}.get(&__caps[0]).unwrap_or(&\"\")).to_string() }}).into_owned()"
            ));
        }
        // `s.gsub(needle, replacement)` — both String args. Ruby
        // returns a new string with all occurrences substituted;
        // Rust's `str::replace(needle, replacement)` is the direct
        // analog (same all-occurrences semantics, returns owned String).
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[0].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let needle_s = emit_expr(&args[0]);
            let repl_s = emit_expr(&args[1]);
            return Some(format!("{recv_s}.replace({needle_s}, {repl_s})"));
        }
        // `arr.length` / `str.length` — Ruby returns Integer.
        // Rust's `.len()` returns `usize`, but Ruby Integers lower
        // to `i64` everywhere else (`while i < arr.length`, `if
        // arr.length == 0`). Emit as `(recv.len() as i64)` on
        // sized receivers so downstream i64 arithmetic / comparison
        // compiles without a per-call-site widen. Untyped receivers
        // fall through to the generic `.length -> .len()` bridge
        // (their value-shape may not even support `.len()`).
        if method == "length"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Hash { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            )
        {
            return Some(format!("({}.len() as i64)", emit_expr(r)));
        }
        // Recv-Ty-aware method bridge — mirrors the structure of
        // `typescript/expr.rs`'s match-on-recv-ty around lines
        // 1972–2245. Each Ruby method on Array/Str/Sym/Hash gets a
        // Rust-shaped lowering; unrecognized names fall through to
        // the generic dispatch + rewrite_method_name table below.
        // Recv-aware so user-defined methods of the same name on
        // other types still resolve through the regular path.
        if let Some(rendered) = dispatch_method_by_recv_ty(r, method, args) {
            return Some(rendered);
        }
        // `str.capitalize` — Ruby's "first letter uppercase, rest
        // lowercase". Rust's String has no direct analog; inline a
        // small block that chains uppercase-first + lowercase-rest.
        // Fires whenever `.capitalize()` is called with no args; the
        // body-typer doesn't always propagate Ty::Str through getter
        // Sends (e.g. `self.model_name.capitalize` where model_name
        // is an attr_reader returning String), so checking recv.ty
        // misses those cases. Non-String receivers would already
        // fail E0599 today and surface as a clearer error after the
        // bridge fires (`&recv_s` deref against a non-String type).
        if method == "capitalize" && args.is_empty() {
            let recv_s = emit_expr(r);
            return Some(format!(
                "{{ let __s: &str = &{recv_s}; let mut __c = __s.chars(); \
                    match __c.next() {{ \
                        Some(__f) => __f.to_uppercase().collect::<String>() + &__c.as_str().to_lowercase(), \
                        None => String::new(), \
                    }} }}"
            ));
        }
        // `value.nil?` on a `Ty::Untyped` or unresolved-Var receiver —
        // `serde_json::Value` exposes `.is_null()` (not `.is_none`,
        // which is the Option method the generic `nil?` bridge below
        // produces). The Var-typed case covers receivers the body-
        // typer didn't fully resolve (e.g. `value = @model[field]`
        // where `@model[field]` is typed Untyped per Base's RBS but
        // the local-let propagation leaves `value`'s recv ty
        // unresolved at the emit-walk's view of the Var-read site).
        // The generic bridge stays in place for true Option-typed
        // receivers (typical Ruby `attr_reader` getters typed `T?`).
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Var { .. })
            )
        {
            return Some(format!("{}.is_null()", emit_expr(r)));
        }
        // `value.nil?` on a non-nilable primitive — body-typer says
        // `Ty::Str` / `Ty::Int` / etc. Ruby/Crystal need the runtime
        // check (Crystal's `property title : String?` is nilable; Ruby
        // attrs default to nil), but rust2's struct field is the bare
        // owned type with no `Option<...>` wrapper, so `.is_none()`
        // would E0599. The static answer here is `false` — emit a
        // constant and let LLVM fold the surrounding If. Side-effect-
        // free recvs (`self.title`) drop out; expression-shaped recvs
        // (a Send chain) need a tiny bind-and-discard, but the body
        // -typer's nil? targets are almost always Ivar reads. Inline
        // the bind only when the recv isn't a pure read.
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(
                    crate::ty::Ty::Str
                        | crate::ty::Ty::Sym
                        | crate::ty::Ty::Int
                        | crate::ty::Ty::Float
                        | crate::ty::Ty::Bool
                )
            )
        {
            let recv_pure = matches!(
                &*r.node,
                ExprNode::Ivar { .. }
                    | ExprNode::Var { .. }
                    | ExprNode::SelfRef
                    | ExprNode::Lit { .. }
                    | ExprNode::Const { .. }
            );
            if recv_pure {
                return Some("false".to_string());
            }
            return Some(format!("{{ let _ = {}; false }}", emit_expr(r)));
        }
        // `self.class.X(args)` — Ruby idiom for "dispatch X on the
        // class of self" (`@id` getter is an instance dispatch;
        // `table_name`, `schema_columns` are per-subclass class
        // methods). The chained Send `recv.class.X` lowers to
        // `Send { recv: Send { recv: SelfRef, method: "class" },
        // method: X }`. In Rust, the equivalent is `Self::X(args)`
        // — the surrounding `impl Base` resolves the per-class
        // override at the call site. Only fires when the recv is
        // SelfRef (subclass overrides reach the correct method
        // through Self resolution); other receivers' .class chains
        // surface as proper E0599 noise upstream.
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
                let rewritten = rewrite_method_name(method);
                let args_s: Vec<String> = args.iter().map(emit_expr).collect();
                if args_s.is_empty() {
                    return Some(format!("Self::{rewritten}()"));
                }
                return Some(format!("Self::{rewritten}({})", args_s.join(", ")));
            }
        }
    None
}
