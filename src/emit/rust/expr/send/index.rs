//! Receiver-aware method bridges. The single `try_recv_typed_method`
//! function probes the `(recv, method, args)` tuple for any of ~15
//! Ruby-method-on-typed-receiver shapes (`[]`, `[]=`, `is_a?`, `to_h`,
//! `merge`, `fetch`, `tr`, `split`, `gsub`, `length`, `capitalize`,
//! `nil?`, `self.class.X`, plus the generic recv-Ty bridge from
//! `dispatch.rs`). Returns `Some(emit)` on a match; `None` to fall
//! through to the method-name-bridge path in `mod.rs`.

use crate::expr::{Expr, ExprNode, Literal};

use super::super::literal::emit_is_a;
use super::super::util::{
    coerce_to_value, is_builtin_container_class, is_copy_ty, peel_nil, rewrite_method_name,
};
use super::super::{
    emit_expr, in_module_singleton, ivar_field_ty, module_singleton_slot_name,
    module_singleton_thread_local, recv_var_back_propagated_hash_kv,
};
use super::dispatch::dispatch_method_by_recv_ty;

pub(super) fn try_recv_typed_method(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    outer_ty: Option<&crate::ty::Ty>,
) -> Option<String> {
    let r = recv?;
    if method == "[]" && args.len() == 1 {
        // Peel `Union<T, Nil>` from the recv Ty so receivers bound
        // via `let x = arr[i]` (typed `T | Nil` by the body-typer's
        // Ruby-semantics view) match the same branches as the
        // plain receiver case. Outside an Option return tail, emit
        // chooses panic-on-miss for `[]`, so the runtime value is T.
        let ivar_fallback = match &*r.node {
            ExprNode::Ivar { name } => ivar_field_ty(name.as_str()),
            _ => None,
        };
        let recv_ty = ivar_fallback.as_ref().or(r.ty.as_ref()).map(peel_nil);
        let arg_ty = args[0].ty.as_ref().map(peel_nil);
        // Ruby's nil-on-miss read only reaches a consumer that takes
        // an Option (the tail of an Option-returning method, or a
        // `let Some(x) = arr[i] else` nil guard); every other site
        // keeps panic-on-miss so it still reads as `T`.
        let option_consumer = super::super::take_option_index_read()
            || (super::super::in_return_tail() && super::super::current_return_is_option());
        let result_is_option =
            outer_ty.is_some_and(super::super::util::is_option_ty) && option_consumer;
        // Range index on Str/Vec receiver — `pp[1..]`. The Range
        // node emits its endpoints unmodified (`1_i64..`), but
        // slice indexing needs `usize`. Wrap the rendered range
        // in a re-cast so `&pp[(1) as usize..]` compiles.
        if matches!(
            recv_ty,
            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym) | Some(crate::ty::Ty::Array { .. })
        ) {
            if let ExprNode::Range {
                begin,
                end,
                exclusive,
            } = &*args[0].node
            {
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
            Some(crate::ty::Ty::Array { .. }) | Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
        ) {
            if let ExprNode::Lit {
                value: Literal::Int { value },
            } = &*args[0].node
            {
                if *value < 0 {
                    let recv_s = emit_expr(r);
                    let abs = -value;
                    // Vec<T>::Index returns `&T`; clone to produce
                    // the owned `T` Ruby's `arr[-1]` semantics
                    // delivers. Callers (e.g. `Base.last`'s tail
                    // wrapped in `Some(...)`) need owned T to
                    // match the `Option<T>` return type.
                    if matches!(recv_ty, Some(crate::ty::Ty::Array { .. })) {
                        if result_is_option {
                            let access = if matches!(
                                recv_ty,
                                Some(crate::ty::Ty::Array { elem }) if is_copy_ty(elem)
                            ) {
                                "copied"
                            } else {
                                "cloned"
                            };
                            return Some(format!(
                                "{{ let __recv = &({recv_s}); __recv.len().checked_sub({abs}_usize).and_then(|__index| __recv.get(__index)).{access}() }}"
                            ));
                        }
                        return Some(format!("{recv_s}[{recv_s}.len() - {abs}_usize].clone()"));
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
                if result_is_option {
                    let access = if is_copy_ty(elem) { "copied" } else { "cloned" };
                    let recv_s = if matches!(&*r.node, ExprNode::Const { .. }) {
                        format!("{}.lock().unwrap()", super::super::emit_send_recv(r))
                    } else {
                        super::super::emit_send_recv(r)
                    };
                    // An element that is itself nilable already is
                    // the Option; a miss and a stored nil both read nil.
                    let flatten = if super::super::util::is_option_ty(elem) {
                        ".flatten()"
                    } else {
                        ""
                    };
                    return Some(format!(
                        "{recv_s}.get(({}) as usize).{access}(){flatten}",
                        emit_expr(&args[0])
                    ));
                }
                let suffix = if is_copy_ty(elem) { "" } else { ".clone()" };
                let recv_s = if matches!(&*r.node, ExprNode::Const { .. }) {
                    format!("{}.lock().unwrap()", super::super::emit_send_recv(r))
                } else {
                    super::super::emit_send_recv(r)
                };
                return Some(format!(
                    "{recv_s}[({}) as usize]{}",
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
                return Some(format!("{}.get({})", emit_expr(r), emit_expr(&args[0])));
            }
            if !is_builtin_container_class(cls) {
                return Some(format!(
                    "{}.get_index({})",
                    emit_expr(r),
                    emit_expr(&args[0])
                ));
            }
        }
        // Ruby's `hash[k]` yields nil for a missing key; Rust's
        // `map[k]` PANICS ("no entry found for key"). Where the map
        // holds `serde_json::Value`, `Value::Null` is that nil, so
        // emit the total form. Previously invisible: every emitted
        // read of an attrs bag carried a `|| <default>` that read
        // through `.get(...).unwrap_or(...)` instead — nullable
        // columns dropped the default and hit the panicking index.
        if map_holds_json_value(recv_ty) {
            if result_is_option {
                return Some(format!("{}.get({}).cloned()", emit_expr(r), emit_expr(&args[0])));
            }
            return Some(format!(
                "{}.get({}).cloned().unwrap_or(serde_json::Value::Null)",
                emit_expr(r),
                emit_expr(&args[0])
            ));
        }
        // Ruby Hash#[] returns nil for a missing key. Rust HashMap's
        // Index implementation panics, so typed Hash reads must use
        // `get` and preserve the Option-shaped result. String and
        // Symbol values both emit as owned Rust Strings, including
        // LazyLock constants whose stored values are `&'static str`.
        if let Some(crate::ty::Ty::Hash { key, value }) = recv_ty {
            let key_s = emit_expr(&args[0]);
            // Normalize either an owned String or borrowed `&str` key
            // to the `str` query type HashMap's Borrow-based lookup
            // expects (not `&String`, which is not Borrow-compatible
            // with a map keyed by `&'static str`).
            let get = format!(
                "{}.get((({key_s}).to_string()).as_str())",
                emit_expr(r)
            );
            if matches!(**key, crate::ty::Ty::Str | crate::ty::Ty::Sym)
                && matches!(**value, crate::ty::Ty::Str | crate::ty::Ty::Sym)
            {
                return Some(format!("{get}.map(|value| value.to_string())"));
            }
            return Some(format!("{get}.cloned()"));
        }
        if matches!(&*r.node, ExprNode::Const { .. }) {
            return Some(format!(
                "{}.lock().unwrap()[({}) as usize]",
                super::super::emit_send_recv(r),
                emit_expr(&args[0]),
            ));
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
        if matches!(
            args[0].ty.as_ref().map(peel_nil),
            Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Regexp"
        ) && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Int))
        {
            let recv_s = emit_expr(r);
            let pattern_s = emit_expr(&args[0]);
            let capture_index = emit_expr(&args[1]);
            return Some(format!(
                "{{ let __source = {recv_s}; let __captures = {pattern_s}.captures(&__source); \
                 __captures.and_then(|__captures| __captures.get(({capture_index}) as usize) \
                 .map(|__capture| __capture.as_str().to_string())) }}"
            ));
        }
        let recv_s = emit_expr(r);
        let start_s = emit_expr(&args[0]);
        let len_s = emit_expr(&args[1]);
        // Ruby semantics, not a raw byte range: characters, a
        // negative start counting from the end, and the slice
        // CLAMPED to the string (`"abc"[1, 10]` is "bc") — a
        // `[a..b]` range panics past the end and on a non-char
        // boundary. Ruby answers nil for a start past the end;
        // the emit is typed String, so that case answers "".
        return Some(format!(
            "{{ let __s: &str = &*({recv_s}); let __n = __s.chars().count() as i64; \
                 let mut __b = ({start_s}) as i64; if __b < 0 {{ __b += __n; }} \
                 let __l = ({len_s}) as i64; \
                 if __b < 0 || __b > __n || __l < 0 {{ String::new() }} \
                 else {{ __s.chars().skip(__b as usize).take(__l as usize).collect::<String>() }} }}"
        ));
    }
    if method == "[]=" && args.len() == 2 {
        // `response.headers[k] = v` — see the LValue::Index arm in
        // `assign.rs`; the Send spelling of the same write.
        if super::super::assign::is_response_headers(r) {
            return Some(format!(
                "crate::http::ResponseHandle.set_header(&({}), ({}).to_string())",
                emit_expr(&args[0]),
                emit_expr(&args[1]),
            ));
        }
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
                if module_singleton_thread_local() {
                    return Some(format!(
                        "{{ {slot}.with(|__s| __s.borrow_mut().get_or_insert_with(std::collections::HashMap::new).insert(({k}).to_string(), ({v}).to_string())); }}"
                    ));
                }
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
                    Some(crate::ty::Ty::Class { id, .. }) => Some(id.0.as_str().to_string()),
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
            let leaf = cls.rsplit("::").next().unwrap_or(cls);
            if leaf == "HeaderStore" {
                // RBS `[]=: (String, String?)` — rust emits
                // `set_index(&str, Option<String>)`, not Value.
                let opt_str = crate::ty::Ty::Union {
                    variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil],
                };
                let wrapped = super::coerce::coerce_arg_for_param_ty(&args[1], &opt_str);
                return Some(format!(
                    "{}.set_index({}, {wrapped})",
                    emit_expr(r),
                    emit_expr(&args[0]),
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
            matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. })) || recv_hash_kv.is_some();
        if is_hash_recv {
            let coerce_str = |slot: Option<&crate::ty::Ty>, raw: String| -> String {
                match slot {
                    Some(crate::ty::Ty::Str | crate::ty::Ty::Sym) => {
                        format!("({raw}).to_string()")
                    }
                    // A heterogeneous slot renders as
                    // `serde_json::Value` (the untyped-hash carrier),
                    // so a `String`-producing arg needs lifting into
                    // it. `Value::from` is the same idiom the
                    // untyped-hash bridge uses at call boundaries,
                    // and it is identity on an arg that is already a
                    // Value — so this stays correct for the mixed
                    // accumulator where some writes are Strings and
                    // others are already Values.
                    Some(t) if super::super::super::ty::rust_ty(t) == "serde_json::Value" => {
                        format!("serde_json::Value::from({raw})")
                    }
                    _ => raw,
                }
            };
            let k = emit_expr(&args[0]);
            let v = emit_expr(&args[1]);
            let (kk, vv) = match &recv_hash_kv {
                Some((k_ty, v_ty)) => (coerce_str(Some(k_ty), k), coerce_str(Some(v_ty), v)),
                None => (k, v),
            };
            // The receiver is a PLACE here, not a value: a defensive
            // `.clone()` (added because this isn't the var's last
            // use) would make the insert land on a temporary and be
            // dropped — the same silent-write-loss as roundhouse#40's
            // each-block case. Strip it; `emit_expr` renders a Var
            // read as exactly `name.clone()`, so the suffix match is
            // precise rather than a guess.
            let recv_s = emit_expr(r);
            let place = recv_s.strip_suffix(".clone()").unwrap_or(&recv_s);
            return Some(format!("{{ {place}.insert({kk}, {vv}); }}"));
        }
        // Vec index-assign, including Const arrays (`FORGERY_SLOT[0]
        // =`) and HeaderStore `@vals[i] =`. Same usize cast as `[]`.
        let ivar_ty = match &*r.node {
            ExprNode::Ivar { name } => ivar_field_ty(name.as_str()),
            _ => None,
        };
        let recv_ty = ivar_ty.as_ref().or(r.ty.as_ref()).map(peel_nil);
        if matches!(recv_ty, Some(crate::ty::Ty::Array { .. }))
            || matches!(&*r.node, ExprNode::Const { .. })
        {
            let recv_s = if matches!(&*r.node, ExprNode::Const { .. }) {
                format!("{}.lock().unwrap()", super::super::emit_send_recv(r))
            } else {
                super::super::emit_send_recv(r)
            };
            return Some(format!(
                "{recv_s}[({}) as usize] = {}",
                emit_expr(&args[0]),
                emit_expr(&args[1]),
            ));
        }
        return Some(format!(
            "{}[{}] = {}",
            emit_expr(r),
            emit_expr(&args[0]),
            emit_expr(&args[1])
        ));
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
        && matches!(
            r.ty.as_ref().map(peel_nil),
            Some(crate::ty::Ty::Hash { .. })
        )
    {
        let key_s = emit_expr(&args[0]);
        let default_is_nil = matches!(
            &*args[1].node,
            ExprNode::Lit {
                value: Literal::Nil
            }
        );
        // Thread-local module-singleton slot (ViewHelpers @slots)
        // — borrow in place instead of cloning the map snapshot
        // out of the slot first.
        if let Some(get_s) = super::super::module_singleton_hash_get(r, &key_s) {
            if default_is_nil {
                return Some(get_s);
            }
            return Some(format!("{get_s}.unwrap_or({})", emit_expr(&args[1])));
        }
        let recv_s = emit_expr(r);
        if default_is_nil {
            return Some(format!("{recv_s}.get({key_s}).cloned()"));
        }
        // Coerce the default against the receiver's value Ty so a
        // `{}` Hash literal default against `HashMap<String, Value>`
        // renders as `Value::Object(...)`, not bare `HashMap::new()`
        // (which trips E0308 at the unwrap_or slot).
        let default_s =
            if let Some(crate::ty::Ty::Hash { value: rv, .. }) = r.ty.as_ref().map(peel_nil) {
                super::coerce::coerce_arg_for_param_ty(&args[1], &rv)
            } else {
                emit_expr(&args[1])
            };
        return Some(format!(
            "{recv_s}.get({key_s}).cloned().unwrap_or({default_s})"
        ));
    }
    // `s.tr(from, to)` — character translation. Limited to
    // single-char from/to (the framework Ruby use case is
    // `inner_k.to_s.tr("_", "-")` in render_attrs). Multi-char
    // and ranges fall through to generic dispatch so the gap
    // surfaces as a compile error rather than silently mis-
    // emitting. Mirrors the TypeScript emit's `tr` peephole.
    if method == "tr" && args.len() == 2 {
        if let (
            ExprNode::Lit {
                value: Literal::Str { value: from },
            },
            ExprNode::Lit {
                value: Literal::Str { value: to },
            },
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
    // indexing. Eagerly collect so downstream `parts.length` /
    // `parts[i]` (the typical router-table walking pattern)
    // compiles. Recv-type-aware: only fires on Ty::Str receivers;
    // user-defined `split` on other types stays intact.
    //
    // OWNED `Vec<String>`, not `Vec<&str>`: the type side already
    // calls this `Array[String]` and declares every field, param
    // and local that holds it `Vec<String>`. Borrowed slices agreed
    // with that only as long as the value never left the expression
    // that produced it — the moment a split result is stored on a
    // struct or passed to a function (a router that splits its
    // patterns once at construction rather than once per request)
    // the two spellings meet and it is an E0308. Owning the
    // segments is also what the borrow checker requires to put them
    // in a field, and it is what Ruby's own `split` does.
    if method == "split" && args.len() == 1 && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Str)) {
        let recv_s = emit_expr(r);
        let sep_s = emit_expr(&args[0]);
        return Some(format!(
            "{recv_s}.split({sep_s}).map(|s| s.to_string()).collect::<Vec<String>>()"
        ));
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
        && matches!(
            args[1].ty.as_ref().map(peel_nil),
            Some(crate::ty::Ty::Hash { .. })
        )
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
    // arr.length == 0`). Emit as `recv.len() as i64` on sized
    // receivers so downstream i64 arithmetic / comparison
    // compiles without a per-call-site widen. The `as` cast is
    // non-primary; decide pass stamps `NEEDS_PARENS` for
    // chained-recv use, so render wraps only where needed.
    // Untyped receivers fall through to the generic `.length
    // -> .len()` bridge (their value-shape may not even
    // support `.len()`).
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
        return Some(format!("{}.len() as i64", super::super::emit_send_recv(r)));
    }
    // Recv-Ty-aware method bridge — mirrors the structure of
    // `typescript/expr.rs`'s match-on-recv-ty around lines
    // 1972–2245. Each Ruby method on Array/Str/Sym/Hash gets a
    // Rust-shaped lowering; unrecognized names fall through to
    // the generic dispatch + rewrite_method_name table below.
    // Recv-aware so user-defined methods of the same name on
    // other types still resolve through the regular path.
    if method == "nil?" && args.is_empty() && rust_emits_non_nil_string_helper(r) {
        return Some(format!("{{ let _ = {}; false }}", emit_expr(r)));
    }
    if method == "nil?" && args.is_empty() && rust_emits_json_value_index(r) {
        return Some(format!("{}.is_null()", emit_expr(r)));
    }
    if method == "nil?" && args.is_empty() && rust_emits_option_local(r) {
        return Some(format!("{}.is_none()", emit_expr(r)));
    }
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
    // Session/Flash `#[]` / `#get` rust-emit as `Option<T>` even
    // when IR types the result as Untyped. `nil?` must use
    // `.is_none()` (the Option API), not `.is_null()` (Value).
    // Same split as `ruby_to_s_emit` for those recvs.
    if method == "nil?" && args.is_empty() && rust_emits_session_or_flash_get(r) {
        return Some(format!("{}.is_none()", emit_expr(r)));
    }
    if method == "nil?" && args.is_empty() {
        if let ExprNode::Send {
            recv: Some(class_recv),
            method: class_method,
            ..
        } = &*r.node
            && let ExprNode::Const { path } = &*class_recv.node
            && let Some(class) = path.last()
            && super::super::global_class_method_return_ty(
                class.as_str(),
                class_method.as_str(),
            )
            .as_ref()
            .is_some_and(super::super::util::is_option_ty)
        {
            return Some(format!("{}.is_none()", emit_expr(r)));
        }
    }
    // `value.nil?` on a receiver that rust emits as
    // `serde_json::Value` — `.is_null()` (not `.is_none`, which
    // is the Option method the generic `nil?` bridge below
    // produces). Untyped/Var are the gradual-escape cases; RBS
    // heterogeneous unions (`String | Integer | Float | bool |
    // nil` on `optional_value_attr`) also render as Value, and
    // must take the same predicate or rustc E0599s. The generic
    // bridge stays in place for true Option-typed receivers
    // (typical Ruby `attr_reader` getters typed `T?`).
    if method == "nil?"
        && args.is_empty()
        && r.ty
            .as_ref()
            .is_some_and(super::super::super::ty::rust_value_shaped)
    {
        return Some(format!("{}.is_null()", emit_expr(r)));
    }
    // `value.nil?` on a non-nilable primitive — body-typer says
    // `Ty::Str` / `Ty::Int` / etc. Ruby/Crystal need the runtime
    // check (Crystal's `property title : String?` is nilable; Ruby
    // attrs default to nil), but rust's struct field is the bare
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

/// Session / Flash `#[]` and `#get` rust-emit as `Option<T>` via the
/// hand-written `.get` shim. IR still types those reads as Untyped.
fn rust_emits_session_or_flash_get(recv: &Expr) -> bool {
    let ExprNode::Send {
        method,
        recv: Some(inner),
        ..
    } = &*recv.node
    else {
        return false;
    };
    if !matches!(method.as_str(), "[]" | "get") {
        return false;
    }
    let Some(crate::ty::Ty::Class { id, .. }) = inner.ty.as_ref().map(peel_nil) else {
        return false;
    };
    let name = id.0.as_str();
    let leaf = name.rsplit("::").next().unwrap_or(name);
    matches!(leaf, "Session" | "Flash")
}

/// True when the receiver is a map whose values render as
/// `serde_json::Value` — the attrs/params bags. Only those have a
/// total nil to fall back on.
fn map_holds_json_value(recv_ty: Option<&crate::ty::Ty>) -> bool {
    use crate::ty::Ty;
    matches!(
        recv_ty,
        Some(Ty::Hash { value, .. })
            if matches!(value.as_ref(), Ty::Untyped)
                || matches!(value.as_ref(), Ty::Union { .. })
    )
}

/// A Ruby hash lookup can have an Optional Ruby-side type while Rust's
/// HashMap bridge materializes the missing value as `Value::Null`. In
/// that case the receiver expression itself is a `serde_json::Value`,
/// so nil? must follow the emitted representation rather than the
/// lookup's Optional source type.
fn rust_emits_json_value_index(expr: &Expr) -> bool {
    let ExprNode::Send {
        recv: Some(map),
        method,
        ..
    } = &*expr.node
    else {
        return false;
    };
    method.as_str() == "[]"
        && matches!(
            map.ty.as_ref(),
            Some(crate::ty::Ty::Hash { value, .. })
                if super::super::super::ty::rust_ty(value) == "serde_json::Value"
        )
}

fn rust_emits_option_local(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Var { .. })
        && expr
            .ty
            .as_ref()
            .is_some_and(|ty| crate::emit::rust::ty::rust_ty(ty).starts_with("Option<"))
}

fn rust_emits_non_nil_string_helper(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Send {
            recv: Some(recv),
            method,
            ..
        } => {
            let ExprNode::Const { path } = &*recv.node else {
                return false;
            };
            let Some(owner) = path.last().map(|name| name.as_str()) else {
                return false;
            };
            match owner {
                "RouteHelpers" => {
                    method.as_str().ends_with("_path") || method.as_str().ends_with("_url")
                }
                "ViewHelpers" => {
                    matches!(method.as_str(), "dom_id" | "asset_path" | "url_for")
                }
                _ => false,
            }
        }
        // View templates express these as bare helper calls in Ruby;
        // the Rust emitter resolves the registered names onto
        // `ViewHelpers::` without changing the IR receiver node.
        ExprNode::Send {
            recv: None, method, ..
        } => {
            matches!(method.as_str(), "dom_id" | "asset_path" | "url_for")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;
    use crate::span::Span;

    #[test]
    fn hash_value_nil_predicate_uses_null_but_option_nil_predicate_is_unchanged() {
        let mut map = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from("attrs"),
            },
        );
        map.ty = Some(crate::ty::Ty::Hash {
            key: Box::new(crate::ty::Ty::Str),
            value: Box::new(crate::ty::Ty::Untyped),
        });
        let mut lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(map),
                method: Symbol::from("[]"),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: "body".to_string(),
                        },
                    },
                )],
                block: None,
                parenthesized: false,
            },
        );
        lookup.ty = Some(crate::ty::Ty::Union {
            variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil],
        });
        let mut option = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(1),
                name: Symbol::from("maybe"),
            },
        );
        option.ty = Some(crate::ty::Ty::Union {
            variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil],
        });
        let option_nil_check = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(option),
                method: Symbol::from("nil?"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
            assert!(rust_emits_json_value_index(&lookup));
            assert_eq!(
                try_recv_typed_method(Some(&lookup), "nil?", &[], None).as_deref(),
                Some("attrs.get(\"body\").cloned().unwrap_or(serde_json::Value::Null).is_null()")
            );
            assert_eq!(
                crate::emit::rust::expr::emit_expr(&option_nil_check),
                "maybe.is_none()"
            );
        });
    }

    #[test]
    fn non_nil_string_helper_nil_check_evaluates_once_then_returns_false() {
        let helper_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("ViewHelpers")],
                    },
                )),
                method: Symbol::from("dom_id"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );

        crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
            assert_eq!(
                try_recv_typed_method(Some(&helper_call), "nil?", &[], None).as_deref(),
                Some("{ let _ = ViewHelpers::dom_id(); false }")
            );
        });
    }

    #[test]
    fn bare_view_string_helper_nil_check_uses_its_emitted_string_type() {
        let helper_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("dom_id"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );

        crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
            assert_eq!(
                try_recv_typed_method(Some(&helper_call), "nil?", &[], None).as_deref(),
                Some("{ let _ = dom_id(); false }")
            );
        });
    }

    #[test]
    fn string_hash_index_uses_a_nil_safe_lookup_for_constants() {
        let mut map = Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("BUILD_VERSIONS")],
            },
        );
        map.ty = Some(crate::ty::Ty::Hash {
            key: Box::new(crate::ty::Ty::Str),
            value: Box::new(crate::ty::Ty::Str),
        });
        let key = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from("build"),
            },
        );
        let mut lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(map),
                method: Symbol::from("[]"),
                args: vec![key],
                block: None,
                parenthesized: false,
            },
        );
        lookup.ty = Some(crate::ty::Ty::Union {
            variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil],
        });

        crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
            assert_eq!(
                crate::emit::rust::expr::emit_expr(&lookup),
                "BUILD_VERSIONS.get(((build).to_string()).as_str()).map(|value| value.to_string())"
            );
        });
    }
}
