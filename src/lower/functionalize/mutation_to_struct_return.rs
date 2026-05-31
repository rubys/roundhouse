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
    if m.receiver != MethodReceiver::Class && should_thread(&m) {
        rewrite(m)
    } else {
        m
    }
}

/// Thread a constructor (`initialize`) body for struct-update emit: the
/// `@field = v` writes become `record` updates and the body returns
/// `record`. The caller (elixir2's `emit_constructor`) seeds
/// `record = %Struct{}` ahead of this. Used only for non-flat
/// constructors (flat `@field = value` ones emit a struct literal
/// directly).
pub fn thread_constructor_body(body: &Expr) -> Expr {
    append_record_return(rewrite_expr(body))
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
    // own value. Replace a trailing `nil` return with `record`.
    m.body = if writes {
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
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::Assign { target: LValue::Ivar { .. }, .. }) {
            found = true;
        }
    });
    found
}

/// Nested mutation (`@flash[:notice] = v` → `Send{recv: Ivar, "[]="}`)
/// or a loop — outside v1; bail. (`yield` IS supported.)
fn has_nested_mutation_or_loop(e: &Expr) -> bool {
    let mut bad = false;
    walk(e, &mut |n| match &*n.node {
        ExprNode::Send { recv: Some(r), method, .. }
            if method.as_str() == "[]=" && matches!(&*r.node, ExprNode::Ivar { .. }) =>
        {
            bad = true
        }
        ExprNode::While { .. } => bad = true,
        _ => {}
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

/// `record.<field>` — a no-arg Send the emitter renders as struct field access.
fn field_read(field: &Symbol) -> Expr {
    syn(ExprNode::Send {
        recv: Some(var(RECORD)),
        method: field.clone(),
        args: vec![],
        block: None,
        parenthesized: false,
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
    fn nested_mutation_bails() {
        // `@flash[:notice] = notice` rides Send `[]=` on an ivar — bail.
        let nested = send(
            Some(syn(ExprNode::Ivar { name: sym("flash") })),
            "[]=",
            vec![syn(ExprNode::Lit { value: Literal::Sym { value: sym("notice") } }), vr("notice")],
        );
        let m = instance_method("redir", &["notice"], syn(ExprNode::Seq { exprs: vec![nested, nil()] }));
        let out = transform_method(m.clone());
        // Unchanged: still has the raw ivar-rooted `[]=` (not threaded).
        assert_eq!(out.body, m.body, "nested mutation should bail (unchanged)");
    }
}
