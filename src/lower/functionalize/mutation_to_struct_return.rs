//! Instance `@ivar` mutation → struct-update return-threading (#29 pass #4).
//!
//! Elixir has no mutable instance state, so an instance method that
//! mutates `@ivar`s must thread a `record` and return the updated copy:
//!
//! ```text
//!   def render(body, status: :ok, …)        def render(record, body, status \\ :ok, …)
//!     @body = body                            record = %{record | body: body}
//!     @status = resolve_status(status)  ──▶   record = %{record | status: resolve_status(status)}
//!     @x = v unless v.nil?                     record = if not is_nil(v), do: %{record | x: v}, else: record
//!     nil                                      record
//!   end                                      end
//! ```
//!
//! Mechanics (see [[project_mutation_threading_pass4]] for the design):
//! - `@x` read → `record.x` (a no-arg `Send` on the `record` var).
//! - `@x = v` write → `record = %{record | x: v}`, bridged through a
//!   synthetic `record.__struct_put__(:x, v)` `Send` that the elixir2
//!   emitter renders as the struct-update literal. (Conditional writes
//!   ride the emitter's existing cond-rebind lift — no work here.)
//! - the method returns `record` (a trailing `nil` is replaced).
//!
//! Pure instance methods (no `@ivar`) are left untouched and take no
//! `record` param (the emitter threads `record` only when the body
//! references it). `initialize` is skipped — the constructor path emits
//! it as `new` returning a struct literal.
//!
//! **v1 scope.** Flat + conditional `@ivar = scalar`. Bails (returns the
//! method unchanged) on nested mutation (`@flash[:notice] = v`, an
//! `[]=`/setter `Send` on an ivar) and on loops/`yield` in the body —
//! those compose with while→recursion and are follow-ups.

use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

const RECORD: &str = "record";

/// Rewrite one method for struct-return threading, or return it
/// unchanged when it isn't an applicable instance mutator.
pub fn transform_method(m: MethodDef) -> MethodDef {
    if m.receiver == MethodReceiver::Class || !should_thread(&m) {
        return m;
    }
    // Mutate-and-return-value (`@x = nil; return v` — returns a value,
    // not self) can't thread to a single struct return; degrade to a
    // documented stub rather than emit a silently-wrong (record-unused)
    // body. Rare (flash#delete); a tuple return is a future option.
    if writes_ivar(&m.body) && has_value_return(&m.body) {
        return stub_mutate_and_return(m);
    }
    rewrite(m)
}

/// True if the body has a `return <non-nil>` — a value the method yields
/// in addition to mutating instance state.
fn has_value_return(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if let ExprNode::Return { value } = &*n.node {
            if !matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                found = true;
            }
        }
    });
    found
}

fn stub_mutate_and_return(mut m: MethodDef) -> MethodDef {
    use crate::dialect::Param;
    let msg = format!(
        "roundhouse: {} mutates instance state and returns a value — \
         unsupported by the Elixir functional lowering",
        m.name.as_str()
    );
    // Params are unused in the stub; `_`-prefix to stay warning-clean.
    m.params = m
        .params
        .iter()
        .map(|p| Param::positional(Symbol::from(format!("_{}", p.as_str()).as_str())))
        .collect();
    m.body = syn(ExprNode::Raise {
        value: syn(ExprNode::Lit { value: Literal::Str { value: msg } }),
    });
    m
}

/// Thread a constructor (`initialize`) body for struct-update emit: the
/// `@field = v` writes become `record` updates and the body returns
/// `record`. The caller (elixir2's `emit_constructor`) seeds
/// `record = %Struct{}` ahead of this. Used only for non-flat
/// constructors (flat `@field = value` ones emit a struct literal
/// directly).
pub fn thread_constructor_body(body: &Expr) -> Expr {
    let rewritten = rewrite_expr(body);
    // A constructor always returns `record`. When the tail is a
    // control-flow `if` (e.g. session's `return if other.nil?` followed
    // by a record-threading populate loop), its value carries the
    // threaded struct — bind it back to `record`, mapping each branch's
    // trailing `nil` (a rewritten bare `return self`) to the unchanged
    // `record`, then return `record`.
    if let ExprNode::Seq { exprs } = &*rewritten.node {
        if let Some(last) = exprs.last() {
            if matches!(&*last.node, ExprNode::If { .. }) && !is_guard_if(last) {
                let mut head: Vec<Expr> = exprs[..exprs.len() - 1].to_vec();
                head.push(syn(ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                    value: tail_nil_to_record(last),
                }));
                head.push(var(RECORD));
                return syn(ExprNode::Seq { exprs: head });
            }
        }
    }
    append_record_return(rewritten)
}

/// Map an expression's trailing value position so a bare `nil` (a
/// rewritten bare `return self` in a constructor) yields `record`.
/// Descends `Seq` tails and both `If` branches; leaves any other tail
/// (e.g. a record-threading loop call) untouched.
fn tail_nil_to_record(e: &Expr) -> Expr {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => var(RECORD),
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            if let Some(last) = exprs.pop() {
                exprs.push(tail_nil_to_record(&last));
            }
            syn(ExprNode::Seq { exprs })
        }
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: tail_nil_to_record(then_branch),
            else_branch: tail_nil_to_record(else_branch),
        }),
        _ => e.clone(),
    }
}

fn should_thread(m: &MethodDef) -> bool {
    // `initialize` is the constructor (emitted as `new`), not a mutator.
    if m.name.as_str() == "initialize" {
        return false;
    }
    // Must touch instance state (`@ivar` or `self`), and stay in coverage.
    touches_self(&m.body) && !has_nested_mutation_or_loop(&m.body)
}

fn rewrite(mut m: MethodDef) -> MethodDef {
    let writes = writes_ivar(&m.body);
    let body = rewrite_expr(&m.body);
    // A mutator returns the updated record; a read-only method keeps its
    // own value. A while→recursion helper (`*__loop`) already returns
    // `record` through its `if` branches (recurse / post-value), so it
    // must NOT get a trailing `record` appended.
    m.body = if writes && !m.name.as_str().ends_with("__loop") {
        append_record_return(body)
    } else {
        body
    };
    m
}

// ---- the rewrite -----------------------------------------------------

fn rewrite_expr(e: &Expr) -> Expr {
    match &*e.node {
        // `@x` read → `record.x`; `self` → `record`.
        ExprNode::Ivar { name } => field_read(name),
        ExprNode::SelfRef => var(RECORD),
        // Rewrite into yielded args (they may read `@ivar`/`self`).
        ExprNode::Yield { args } => {
            syn(ExprNode::Yield { args: args.iter().map(rewrite_expr).collect() })
        }
        // `@x = v` write → `record = record.__struct_put__(:x, v)`.
        ExprNode::Assign { target: LValue::Ivar { name }, value } => syn(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
            value: struct_put(name, rewrite_expr(value)),
        }),
        // `@x[k] = v` → `record = %{record | x: Map.put(record.x, k, v)}`,
        // bridged: a struct-put of field `x` to (record.x with k→v). The
        // emitter renders __struct_put__ as `%{record | x: …}` and
        // __index_put__ on the (Hash) field as `Map.put`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Ivar { name } = &*r.node {
                let updated_field = syn(ExprNode::Send {
                    recv: Some(field_read(name)),
                    method: Symbol::from("__index_put__"),
                    args: vec![rewrite_expr(&args[0]), rewrite_expr(&args[1])],
                    block: None,
                    parenthesized: true,
                });
                return syn(ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                    value: struct_put(name, updated_field),
                });
            }
            // Non-ivar `[]=` (e.g. on a local) is left for local_accumulation.
            syn(ExprNode::Send {
                recv: Some(rewrite_expr(r)),
                method: method.clone(),
                args: args.iter().map(rewrite_expr).collect(),
                block: None,
                parenthesized: true,
            })
        }
        // Recurse through the container variants the runtime bodies use.
        ExprNode::Seq { exprs } => syn(ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_expr).collect(),
        }),
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: rewrite_expr(cond),
            then_branch: rewrite_expr(then_branch),
            else_branch: rewrite_expr(else_branch),
        }),
        ExprNode::Send { recv, method, args, block, parenthesized } => syn(ExprNode::Send {
            recv: recv.as_ref().map(rewrite_expr),
            method: method.clone(),
            args: args.iter().map(rewrite_expr).collect(),
            block: block.as_ref().map(rewrite_expr),
            parenthesized: *parenthesized,
        }),
        ExprNode::BoolOp { op, surface, left, right } => syn(ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_expr(left),
            right: rewrite_expr(right),
        }),
        ExprNode::Assign { target, value } => syn(ExprNode::Assign {
            target: target.clone(),
            value: rewrite_expr(value),
        }),
        // A bare `return` (return self/nil) in a threaded method returns
        // the updated `record`; `return expr` threads the expr.
        ExprNode::Return { value } => {
            if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                syn(ExprNode::Return { value: var(RECORD) })
            } else {
                syn(ExprNode::Return { value: rewrite_expr(value) })
            }
        }
        ExprNode::Array { elements, style } => syn(ExprNode::Array {
            elements: elements.iter().map(rewrite_expr).collect(),
            style: *style,
        }),
        ExprNode::Hash { entries, kwargs } => syn(ExprNode::Hash {
            entries: entries.iter().map(|(k, v)| (rewrite_expr(k), rewrite_expr(v))).collect(),
            kwargs: *kwargs,
        }),
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            syn(ExprNode::StringInterp {
                parts: parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite_expr(expr) },
                        InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    })
                    .collect(),
            })
        }
        ExprNode::Cast { value, target_ty } => syn(ExprNode::Cast {
            value: rewrite_expr(value),
            target_ty: target_ty.clone(),
        }),
        // Recurse into block bodies (a block may read `@ivar`/`self`).
        ExprNode::Lambda { params, block_param, body, block_style } => syn(ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_expr(body),
            block_style: *block_style,
        }),
        // Leaves and variants v1 doesn't expect inside a threaded body.
        _ => e.clone(),
    }
}

/// Replace a trailing `nil` with `record`, or append `record`, so the
/// method yields the updated struct.
fn append_record_return(body: Expr) -> Expr {
    let record = var(RECORD);
    // Already returns `record` (e.g. a rewritten trailing `self`)? Leave it.
    if is_record_var(&body) {
        return body;
    }
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            match exprs.last() {
                // Already returns `record` (self) — leave it.
                Some(e) if is_record_var(e) => return body,
                // Tail is a guard `if` (`return X if c` → then-branch
                // returns) — it's the terminal value (both branches yield
                // record), so don't append. (Constructor-with-loop:
                // `if is_nil(other) do record else …__loop(record,…) end`.)
                Some(e) if is_guard_if(e) => return body,
                // Drop a trailing `nil` or a bare local-var read: in a
                // threaded mutator the method's own return value is
                // superseded by `record` (e.g. `[]=` returns the assigned
                // `value`, which is dead once we return the struct).
                Some(e)
                    if matches!(
                        &*e.node,
                        ExprNode::Lit { value: Literal::Nil } | ExprNode::Var { .. }
                    ) =>
                {
                    exprs.pop();
                }
                _ => {}
            }
            exprs.push(record);
            syn(ExprNode::Seq { exprs })
        }
        ExprNode::Lit { value: Literal::Nil } => record,
        _ => syn(ExprNode::Seq { exprs: vec![body, record] }),
    }
}

/// True when `e` is an `if` whose then-branch ends in a `return` — a
/// guard that produces the method's value (so it shouldn't get a
/// trailing `record` appended after it).
fn is_guard_if(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::If { then_branch, .. } if ends_in_return(then_branch))
}

fn ends_in_return(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Return { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(ends_in_return),
        _ => false,
    }
}

fn is_record_var(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == RECORD)
}

// ---- detection -------------------------------------------------------

fn touches_self(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::Ivar { .. } | ExprNode::SelfRef)
            || matches!(&*n.node, ExprNode::Assign { target: LValue::Ivar { .. }, .. })
        {
            found = true;
        }
    });
    found
}

fn writes_ivar(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| match &*n.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, .. } => found = true,
        // Nested `@x[k] = v` (an ivar-rooted `[]=`) also mutates state.
        ExprNode::Send { recv: Some(r), method, .. }
            if method.as_str() == "[]=" && matches!(&*r.node, ExprNode::Ivar { .. }) =>
        {
            found = true
        }
        _ => {}
    });
    found
}

/// Nested mutation (`@flash[:notice] = v` → `Send{recv: Ivar, "[]="}`)
/// or a loop — outside v1; bail. (`yield` IS supported.)
/// A `while` loop in an instance method — not yet threaded (the
/// recursion would need to carry `record`). `each`/`initialize` bail
/// here for now. (Nested `@x[k] = v` IS handled — see `rewrite_expr`.)
fn has_nested_mutation_or_loop(e: &Expr) -> bool {
    let mut bad = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::While { .. }) {
            bad = true;
        }
    });
    bad
}

// ---- IR builders + walker -------------------------------------------

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn var(name: &str) -> Expr {
    syn(ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

/// `@field` read → `record.__field__(:field)`, a bridge the emitter
/// renders as `record.field` (struct field access). Using a bridge —
/// rather than a bare `record.field` Send — keeps field reads
/// unambiguous from method calls (`record.to_h`), which the emitter
/// routes to a same-module function.
fn field_read(field: &Symbol) -> Expr {
    syn(ExprNode::Send {
        recv: Some(var(RECORD)),
        method: Symbol::from("__field__"),
        args: vec![syn(ExprNode::Lit { value: Literal::Sym { value: field.clone() } })],
        block: None,
        parenthesized: true,
    })
}

/// `record.__struct_put__(:field, value)` — the emitter renders this as
/// `%{record | field: value}`.
fn struct_put(field: &Symbol, value: Expr) -> Expr {
    let field_sym = syn(ExprNode::Lit { value: Literal::Sym { value: field.clone() } });
    syn(ExprNode::Send {
        recv: Some(var(RECORD)),
        method: Symbol::from("__struct_put__"),
        args: vec![field_sym, value],
        block: None,
        parenthesized: true,
    })
}

fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &*e.node {
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            exprs.iter().for_each(|x| walk(x, f))
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, f);
            }
            args.iter().for_each(|a| walk(a, f));
            if let Some(b) = block {
                walk(b, f);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, f);
            walk(then_branch, f);
            walk(else_branch, f);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, f);
            walk(body, f);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, f);
            walk(right, f);
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => walk(value, f),
        ExprNode::Return { value } | ExprNode::Raise { value } | ExprNode::Cast { value, .. } => {
            walk(value, f)
        }
        ExprNode::Yield { args } => args.iter().for_each(|a| walk(a, f)),
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(k, v)| {
            walk(k, f);
            walk(v, f);
        }),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, f);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, LibraryClass, Param};
    use crate::effect::EffectSet;
    use crate::ident::ClassId;

    fn sym(s: &str) -> Symbol {
        Symbol::from(s)
    }
    fn vr(name: &str) -> Expr {
        var(name)
    }
    fn nil() -> Expr {
        syn(ExprNode::Lit { value: Literal::Nil })
    }
    fn assign_ivar(name: &str, value: Expr) -> Expr {
        syn(ExprNode::Assign { target: LValue::Ivar { name: sym(name) }, value })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        syn(ExprNode::Send {
            recv,
            method: sym(method),
            args,
            block: None,
            parenthesized: false,
        })
    }
    fn if_(cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
        syn(ExprNode::If { cond, then_branch, else_branch })
    }
    fn instance_method(name: &str, params: &[&str], body: Expr) -> MethodDef {
        MethodDef {
            name: sym(name),
            receiver: MethodReceiver::Instance,
            params: params.iter().map(|p| Param::positional(sym(p))).collect(),
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        }
    }
    fn render_via_elixir(methods: Vec<MethodDef>) -> String {
        let class = LibraryClass {
            name: ClassId(sym("Ctl")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods,
            origin: None,
        };
        crate::emit::elixir2::emit_library_class(&class).expect("emit")
    }

    /// `def render(body, content_type)
    ///   @body = body
    ///   @content_type = content_type unless content_type.nil?
    ///   nil
    /// end`
    fn render_method() -> MethodDef {
        let cond_write = if_(
            send(Some(vr("content_type")), "nil?", vec![]),
            nil(),
            assign_ivar("content_type", vr("content_type")),
        );
        let body = syn(ExprNode::Seq {
            exprs: vec![assign_ivar("body", vr("body")), cond_write, nil()],
        });
        instance_method("render", &["body", "content_type"], body)
    }

    #[test]
    fn mutator_threads_record_and_returns_it() {
        let out = transform_method(render_method());
        let ex = render_via_elixir(vec![out]);
        eprintln!("--- render ---\n{ex}\n--------------");
        assert!(ex.contains("def render(record, body, content_type)"), "threads record:\n{ex}");
        assert!(ex.contains("record = %{record | body: body}"), "flat write:\n{ex}");
        // conditional write (`unless`) → else-side cond-rebind lift.
        assert!(
            ex.contains("record = if is_nil(content_type) do\n      record\n    else\n      %{record | content_type: content_type}"),
            "conditional write lifts:\n{ex}"
        );
        // returns the updated record (trailing nil replaced).
        let trimmed = ex.trim_end();
        assert!(trimmed.ends_with("record\n  end\nend"), "returns record:\n{ex}");
        assert!(!ex.contains("@"), "no raw ivars remain:\n{ex}");
    }

    #[test]
    fn nested_ivar_index_assign_threads_via_map_put() {
        // def []=(key, value); @data[key] = value; end
        let assign = send(
            Some(syn(ExprNode::Ivar { name: sym("data") })),
            "[]=",
            vec![vr("key"), vr("value")],
        );
        let m = instance_method("[]=", &["key", "value"], syn(ExprNode::Seq { exprs: vec![assign] }));
        let ex = render_via_elixir(vec![transform_method(m)]);
        eprintln!("--- nested []= ---\n{ex}\n------------------");
        assert!(ex.contains("def put(record, key, value)"), "[]= → put, threaded:\n{ex}");
        assert!(
            ex.contains("record = %{record | data: Map.put(record.data, key, value)}"),
            "nested @data[k]=v → struct-update with Map.put:\n{ex}"
        );
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    #[test]
    fn yielding_method_takes_block_fn_and_returns_record() {
        // `def each; yield @notice; self; end` (Instance).
        let body = syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Yield { args: vec![syn(ExprNode::Ivar { name: sym("notice") })] }),
                syn(ExprNode::SelfRef),
            ],
        });
        let out = transform_method(instance_method("each", &[], body));
        let ex = render_via_elixir(vec![out]);
        eprintln!("--- each ---\n{ex}\n------------");
        assert!(ex.contains("def each(record, block_fn)"), "threads record + block_fn:\n{ex}");
        assert!(ex.contains("block_fn.(record.notice)"), "yield → block_fn call:\n{ex}");
        assert!(!ex.contains("self") && !ex.contains("@"), "self/@ rewritten:\n{ex}");
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    #[test]
    fn index_operator_def_and_self_call_route_to_get() {
        // def [](key); @notice; end   →  def get(record, key) do record.notice end
        // def fetch(key); self[key]; end → def fetch(record, key) do get(record, key) end
        let getter = instance_method(
            "[]",
            &["key"],
            syn(ExprNode::Seq { exprs: vec![syn(ExprNode::Ivar { name: sym("notice") })] }),
        );
        let self_index = send(
            Some(syn(ExprNode::SelfRef)),
            "[]",
            vec![vr("key")],
        );
        let fetch = instance_method("fetch", &["key"], syn(ExprNode::Seq { exprs: vec![self_index] }));
        let ex = render_via_elixir(vec![transform_method(getter), transform_method(fetch)]);
        eprintln!("--- index ops ---\n{ex}\n-----------------");
        assert!(ex.contains("def get(record, key)"), "[] def → get:\n{ex}");
        assert!(ex.contains("record.notice"), "getter reads field:\n{ex}");
        assert!(ex.contains("def fetch(record, key)"), "fetch threaded:\n{ex}");
        assert!(ex.contains("get(record, key)"), "self[key] → get(record, key):\n{ex}");
    }

    #[test]
    fn if_elsif_chain_reassignment_threads() {
        // def set(key, value)
        //   if key == "notice" then @notice = value
        //   elsif key == "alert" then @alert = value end
        // end
        let chain = if_(
            send(Some(vr("key")), "==", vec![str_lit_helper("notice")]),
            assign_ivar("notice", vr("value")),
            if_(
                send(Some(vr("key")), "==", vec![str_lit_helper("alert")]),
                assign_ivar("alert", vr("value")),
                nil(),
            ),
        );
        let m = instance_method("set", &["key", "value"], syn(ExprNode::Seq { exprs: vec![chain] }));
        let ex = render_via_elixir(vec![transform_method(m)]);
        eprintln!("--- if-chain ---\n{ex}\n----------------");
        assert!(ex.contains("record = if key == \"notice\" do"), "chain lifts to record =:\n{ex}");
        assert!(ex.contains("%{record | notice: value}"), "then update:\n{ex}");
        assert!(ex.contains("%{record | alert: value}"), "elsif update:\n{ex}");
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    fn str_lit_helper(s2: &str) -> Expr {
        syn(ExprNode::Lit { value: Literal::Str { value: s2.to_string() } })
    }

    #[test]
    fn non_flat_constructor_threads_a_seeded_record() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        // def initialize(other = nil)
        //   @notice = nil
        //   return if other.nil?
        //   @notice = other
        // end
        let body = syn(ExprNode::Seq {
            exprs: vec![
                assign_ivar("notice", nil()),
                if_(send(Some(vr("other")), "nil?", vec![]), syn(ExprNode::Return { value: nil() }), nil()),
                assign_ivar("notice", vr("other")),
            ],
        });
        let init = MethodDef {
            name: sym("initialize"),
            receiver: MethodReceiver::Instance,
            params: vec![Param::with_default(sym("other"), nil())],
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        };
        // initialize isn't threaded by the pipeline; emit_constructor calls
        // thread_constructor_body itself. Render it through a class.
        let class = LibraryClass {
            name: ClassId(sym("Flash")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![init],
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- ctor ---\n{ex}\n------------");
        assert!(ex.contains("def new(other \\\\ nil)"), "default param:\n{ex}");
        assert!(ex.contains("record = %V2.Flash{}"), "seeds struct:\n{ex}");
        assert!(ex.contains("record = %{record | notice: nil}"), "threaded write:\n{ex}");
        assert!(ex.contains("if is_nil(other) do"), "guard:\n{ex}");
        assert!(!ex.contains("@"), "no raw ivars:\n{ex}");
    }

    #[test]
    fn pure_instance_method_takes_no_record() {
        // `def resolve_status(s); s; end` — no @ivar, so no record param.
        let m = instance_method("resolve_status", &["s"], syn(ExprNode::Seq { exprs: vec![vr("s")] }));
        let out = transform_method(m);
        let ex = render_via_elixir(vec![out]);
        assert!(ex.contains("def resolve_status(s)"), "no record param:\n{ex}");
        assert!(!ex.contains("record"), "pure method untouched:\n{ex}");
    }

    #[test]
    fn instance_method_with_while_bails() {
        // A `while` in an instance method isn't threaded yet (the
        // recursion would need to carry `record`) — left unchanged.
        let loop_ = syn(ExprNode::While {
            cond: send(Some(vr("i")), "<", vec![vr("n")]),
            body: syn(ExprNode::Seq { exprs: vec![assign_ivar("data", vr("i"))] }),
            until_form: false,
        });
        let m = instance_method("walk", &["n"], syn(ExprNode::Seq { exprs: vec![loop_, nil()] }));
        let out = transform_method(m.clone());
        assert_eq!(out.body, m.body, "instance-method while should bail (unchanged)");
    }
}
