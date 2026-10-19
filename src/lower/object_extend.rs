//! `obj.extend Mod` on an INSTANCE mixes `Mod`'s instance methods into
//! that one object. Spinel has no runtime method table; it links the
//! call only when it can see the receiver's single write is
//! `<UserClass>.new(...)` and `Mod` is a module it can copy onto a
//! synthesized subclass (`register_singleton_defs` in matz/spinel).
//! The subclass is the per-object method table: `class` stays the
//! original class, a sibling `.new` does not receive the methods, and
//! a later call — including one inside a method of the parent class —
//! dispatches to the copy after the `extend` statement flips the
//! object's class id.
//!
//! Two source shapes hide that write from Spinel, and both show up in
//! campfire's TLS-failure test:
//!
//! * `Module.new { private def begin_transport(*) = raise ... }` — a
//!   `def` in the block is not a module body. Ingest stamps it
//!   `nested def` and Spinel refuses a `def` inside `Module.new`.
//! * `pinned_connection(server).extend(mod)` — the receiver is a call,
//!   and the helper returns `HTTP.new(...).tap { ... }`, which is not
//!   itself a `.new` write.
//!
//! When the block is only method definitions and the receiver explodes
//! to one `.new` of a class this program declares (not an ActiveRecord
//! model), the module is hoisted to a real `module` and the receiver
//! becomes a local whose only write is that `.new`. The `extend` stays
//! in the IR, so MRI and Spinel both run it. Anything else is the
//! raise stub this file has always written: one leftover `.extend`
//! keeps a whole Spinel test file from linking, so a test that still
//! has one is replaced wholesale.
//!
//! `extend Foo` with no receiver, or `Foo.extend Bar` on a constant,
//! is the class-body form. Ingest and `fold_extended_modules` own it.

use std::collections::HashSet;

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity, NESTED_DEF};
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, MethodVisibility, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::{SourceFile, Span};

const CONSTRUCT: &str = "Object#extend";
const DETAIL: &str = "mixing a module into one live object needs a per-object method \
                      table, which no compiled target has; the site raises";

pub fn apply_object_extend_stub(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| stub_sites(e, &mut diags));
    for view in &mut app.views {
        stub_sites(&mut view.body, &mut diags);
    }
    let sources = app.sources.clone();
    let library = library_index(app);
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            stub_sites(setup, &mut diags);
        }
        let helpers = factories(tm);
        let mut classes = library.classes.clone();
        let mut base_modules = library.modules.clone();
        let mut extend_defined = library.extend_defined.clone();
        for lc in &tm.inner_classes {
            note_class(lc, &library.models, &mut classes, &mut base_modules);
            let parent_defines = lc
                .parent
                .as_ref()
                .is_some_and(|p| extend_defined.contains(p.0.as_str()));
            if defines_instance_extend(lc) || parent_defines {
                extend_defined.insert(lc.name.0.as_str().to_string());
            }
        }
        for t in &mut tm.tests {
            let mut modules = base_modules.clone();
            let added =
                hoist_anonymous_modules(&mut t.body, &sources, &mut tm.inner_classes, &mut modules);
            let mut seq = 0u32;
            rewrite_scope(&mut t.body, &classes, &modules, &helpers, &mut seq);
            if let Some(d) = first_untraced(&t.body, &classes, &modules, &extend_defined) {
                let keep = tm.inner_classes.len() - added;
                tm.inner_classes.truncate(keep);
                diags.push(d);
                t.body = stub_expr(t.body.span);
            }
        }
        for m in &mut tm.helpers {
            stub_sites(&mut m.body, &mut diags);
        }
    }
    diags
}

struct LibraryIndex {
    models: HashSet<String>,
    /// Classes this program declares that are not ActiveRecord models.
    /// Spinel builds the singleton subclass from the `.new` receiver,
    /// and a model instance is not that layout.
    classes: HashSet<String>,
    /// Modules whose instance methods can be copied. A module that
    /// defines `self.extended` is refused by Spinel: the hook would
    /// have to run at the call.
    modules: HashSet<String>,
    /// Classes that define their own `extend`. The call is that method,
    /// and its return value is not the receiver.
    extend_defined: HashSet<String>,
}

fn library_index(app: &App) -> LibraryIndex {
    let models: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    let mut classes = HashSet::new();
    let mut modules = HashSet::new();
    for lc in &app.library_classes {
        note_class(lc, &models, &mut classes, &mut modules);
    }
    let extend_defined = extend_defined_names(&app.library_classes);
    LibraryIndex {
        models,
        classes,
        modules,
        extend_defined,
    }
}

fn defines_instance_extend(lc: &LibraryClass) -> bool {
    lc.methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "extend")
}

/// A class, and any class under it, whose `extend` is not `Object#extend`.
fn extend_defined_names(classes: &[LibraryClass]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut grew = true;
    while grew {
        grew = false;
        for lc in classes {
            let name = lc.name.0.as_str().to_string();
            if out.contains(&name) {
                continue;
            }
            let parent_defines = lc
                .parent
                .as_ref()
                .is_some_and(|p| out.contains(p.0.as_str()));
            if defines_instance_extend(lc) || parent_defines {
                out.insert(name);
                grew = true;
            }
        }
    }
    out
}

fn note_class(
    lc: &LibraryClass,
    models: &HashSet<String>,
    classes: &mut HashSet<String>,
    modules: &mut HashSet<String>,
) {
    let name = lc.name.0.as_str();
    if lc.is_module {
        if !defines_extended(lc) {
            modules.insert(name.to_string());
        }
    } else if !models.contains(&name.to_string()) {
        classes.insert(name.to_string());
    }
}

fn defines_extended(lc: &LibraryClass) -> bool {
    lc.methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == "extended")
}

struct Factory {
    name: Symbol,
    params: Vec<Param>,
    body: Expr,
    owner: ClassId,
    /// Bare constants of `owner` that a body copied onto the test
    /// class would no longer resolve lexically.
    constants: Vec<Symbol>,
}

fn factories(tm: &crate::dialect::TestModule) -> Vec<Factory> {
    let mut out = Vec::new();
    for m in tm
        .helpers
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Instance)
    {
        out.push(Factory {
            name: m.name.clone(),
            params: m.params.clone(),
            body: m.body.clone(),
            owner: tm.name.clone(),
            constants: Vec::new(),
        });
    }
    for inc in &tm.includes {
        let Some(lc) = tm.inner_classes.iter().find(|c| &c.name == inc) else {
            continue;
        };
        let constants: Vec<Symbol> = lc.constants.iter().map(|(n, _)| n.clone()).collect();
        for m in lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Instance)
        {
            if out.iter().any(|f| f.name == m.name) {
                continue;
            }
            out.push(Factory {
                name: m.name.clone(),
                params: m.params.clone(),
                body: m.body.clone(),
                owner: lc.name.clone(),
                constants: constants.clone(),
            });
        }
    }
    out
}

/// `name = Module.new { def ... }` becomes a real module. The local's
/// reads in `extend` arguments become that module's constant. Returns
/// how many modules were appended to `inner`.
fn hoist_anonymous_modules(
    body: &mut Expr,
    sources: &[SourceFile],
    inner: &mut Vec<LibraryClass>,
    modules: &mut HashSet<String>,
) -> usize {
    let mut found = Vec::new();
    collect_module_assigns(body, sources, &mut found);
    let before = inner.len();
    let mut n = 0u32;
    for (local, methods) in found {
        if !sole_extend_local(body, &local) {
            continue;
        }
        let name = fresh_const(&mut n, |c| {
            inner.iter().any(|lc| lc.name.0.as_str() == c) || modules.contains(c)
        });
        let id = ClassId(Symbol::from(name.as_str()));
        let mut defs = Vec::new();
        let mut ok = true;
        for (vis, span) in &methods {
            let Some(text) = source_slice(sources, *span) else {
                ok = false;
                break;
            };
            let Some(mut def) = method_from_source(text, &id) else {
                ok = false;
                break;
            };
            def.visibility = *vis;
            defs.push(def);
        }
        if !ok || defs.is_empty() {
            continue;
        }
        // `self.extended` runs at the call. Spinel refuses the module,
        // then refuses `Object#extend`, and the test file does not link.
        if defs
            .iter()
            .any(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == "extended")
        {
            continue;
        }
        inner.push(LibraryClass {
            name: id,
            is_module: true,
            parent: None,
            parent_span: Span::synthetic(),
            includes: Vec::new(),
            include_spans: Vec::new(),
            methods: defs,
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
            class_ivar_initializers: Vec::new(),
        });
        modules.insert(name.clone());
        let constant = Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from(name.as_str())],
            },
        );
        replace_extend_arg(body, &local, &constant);
        delete_assign(body, &local);
    }
    inner.len() - before
}

fn fresh_const(n: &mut u32, taken: impl Fn(&str) -> bool) -> String {
    loop {
        *n += 1;
        let name = format!("RhInstanceExtend{n}");
        if !taken(&name) {
            return name;
        }
    }
}

fn collect_module_assigns(
    expr: &Expr,
    sources: &[SourceFile],
    out: &mut Vec<(Symbol, Vec<(MethodVisibility, Span)>)>,
) {
    if let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &*expr.node
    {
        if let Some(methods) = module_new_methods(value) {
            if methods
                .iter()
                .all(|(_, span)| source_slice(sources, *span).is_some())
            {
                out.push((name.clone(), methods));
            }
        }
    }
    expr.node
        .for_each_child(&mut |c| collect_module_assigns(c, sources, out));
}

fn module_new_methods(expr: &Expr) -> Option<Vec<(MethodVisibility, Span)>> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block: Some(block),
        ..
    } = &*expr.node
    else {
        return None;
    };
    if method.as_str() != "new" || !args.is_empty() || !const_is(recv, "Module") {
        return None;
    }
    let ExprNode::Lambda {
        params,
        rest_param,
        extra_params,
        body,
        ..
    } = &*block.node
    else {
        return None;
    };
    if !params.is_empty() || rest_param.is_some() || !extra_params.is_empty() {
        return None;
    }
    let mut vis = MethodVisibility::Public;
    let mut out = Vec::new();
    for stmt in statements(body) {
        match &*stmt.node {
            ExprNode::Send {
                recv: None,
                method,
                args,
                block: None,
                ..
            } if args.is_empty() && visibility_name(method.as_str()).is_some() => {
                vis = visibility_name(method.as_str()).unwrap();
            }
            ExprNode::Send {
                recv: None,
                method,
                args,
                block: None,
                ..
            } if args.len() == 1 && is_nested_def(&args[0]) => {
                let Some(v) = visibility_name(method.as_str()) else {
                    return None;
                };
                out.push((v, args[0].span));
            }
            _ if is_nested_def(stmt) => out.push((vis, stmt.span)),
            _ => return None,
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn visibility_name(name: &str) -> Option<MethodVisibility> {
    match name {
        "private" => Some(MethodVisibility::Private),
        "protected" => Some(MethodVisibility::Protected),
        "public" => Some(MethodVisibility::Public),
        _ => None,
    }
}

fn is_nested_def(expr: &Expr) -> bool {
    matches!(expr.diagnostic(), Some(DiagnosticKind::Unsupported { construct, .. }) if construct.as_str() == NESTED_DEF)
}

fn sole_extend_local(body: &Expr, name: &Symbol) -> bool {
    let mut writes = 0u32;
    let reads = std::cell::Cell::new(0u32);
    let mut extend_reads = 0u32;
    let mut foreign = false;
    let total = extend_reads_of(body, name);
    visit_scoped(
        body,
        &|e| total > 0 && extend_reads_of(e, name) == total,
        &mut |e| {
            if is_plain_assign_to(e, name) {
                writes += 1;
            } else if is_other_write(e, name) {
                writes += 2;
            }
            if is_extend_read(e, name) {
                extend_reads += 1;
            }
            if var_is(e, name) {
                reads.set(reads.get() + 1);
            }
        },
        &mut |e| {
            if is_plain_assign_to(e, name) || is_other_write(e, name) {
                foreign = true;
            }
            // A read in a scope that does not extend still uses the
            // local. Deleting the assignment would leave a NameError.
            if var_is(e, name) {
                reads.set(reads.get() + 1);
            }
        },
    );
    let reads = reads.get();
    !foreign && writes == 1 && reads > 0 && reads == extend_reads
}

fn is_extend_read(expr: &Expr, name: &Symbol) -> bool {
    matches!(&*expr.node, ExprNode::Send { method, args, .. }
        if method.as_str() == "extend" && args.len() == 1 && var_is(&args[0], name))
}

fn extend_reads_of(expr: &Expr, name: &Symbol) -> u32 {
    let mut n = 0u32;
    walk(expr, &mut |e| {
        if is_extend_read(e, name) {
            n += 1;
        }
    });
    n
}

/// A gate or a block is its own scope. Enter it only when `contains`
/// says the extend being judged is inside. A write in a skipped scope
/// is `on_foreign`: it is not the one unconditional definition.
fn visit_scoped(
    expr: &Expr,
    contains: &impl Fn(&Expr) -> bool,
    on_node: &mut impl FnMut(&Expr),
    on_foreign: &mut impl FnMut(&Expr),
) {
    let boundary = gates_control(expr) || matches!(&*expr.node, ExprNode::Lambda { .. });
    if boundary && !contains(expr) {
        walk(expr, on_foreign);
        return;
    }
    on_node(expr);
    expr.node.for_each_child(&mut |child| {
        visit_scoped(child, contains, on_node, on_foreign);
    });
}

fn contains_expr(hay: &Expr, needle: &Expr) -> bool {
    if std::ptr::eq(hay, needle) {
        return true;
    }
    let mut found = false;
    hay.node.for_each_child(&mut |child| {
        if !found {
            found = contains_expr(child, needle);
        }
    });
    found
}

fn replace_extend_arg(expr: &mut Expr, name: &Symbol, constant: &Expr) {
    if let ExprNode::Send { method, args, .. } = &mut *expr.node {
        if method.as_str() == "extend" && args.len() == 1 && var_is(&args[0], name) {
            args[0] = constant.clone();
        }
    }
    expr.node
        .for_each_child_mut(&mut |c| replace_extend_arg(c, name, constant));
}

fn delete_assign(expr: &mut Expr, name: &Symbol) {
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        exprs.retain(|e| !is_plain_assign_to(e, name));
    }
    if is_plain_assign_to(expr, name) {
        *expr = Expr::new(
            expr.span,
            ExprNode::Lit {
                value: Literal::Nil,
            },
        );
        return;
    }
    expr.node
        .for_each_child_mut(&mut |c| delete_assign(c, name));
}

/// Statements of one scope — the test method, or a block body — gain a
/// local for each `extend` whose receiver explodes to a single `.new`.
fn rewrite_scope(
    body: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
) {
    let span = body.span;
    let mut exprs = if let ExprNode::Seq { exprs } = &mut *body.node {
        std::mem::take(exprs)
    } else {
        vec![body.clone()]
    };
    let mut i = 0;
    while i < exprs.len() {
        rewrite_lambdas(&mut exprs[i], classes, modules, helpers, seq);
        let setups = pull_setups(&mut exprs[i], classes, modules, helpers, seq, false);
        let n = setups.len();
        if n > 0 {
            for (k, setup) in setups.into_iter().enumerate() {
                exprs.insert(i + k, setup);
            }
            i += n;
        }
        i += 1;
    }
    *body = if exprs.len() == 1 {
        exprs.pop().unwrap()
    } else {
        Expr::new(span, ExprNode::Seq { exprs })
    };
}

fn rewrite_lambdas(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
) {
    if matches!(&*expr.node, ExprNode::Lambda { .. }) {
        if let ExprNode::Lambda { body, .. } = &mut *expr.node {
            rewrite_scope(body, classes, modules, helpers, seq);
        }
        return;
    }
    expr.node
        .for_each_child_mut(&mut |c| rewrite_lambdas(c, classes, modules, helpers, seq));
}

fn pull_setups(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
) -> Vec<Expr> {
    // Hoisting in front of this statement would run the extend before
    // an effect that Ruby evaluates first (`foo(bar(), obj.extend(mod))`).
    if effect_precedes_extend(expr) {
        return Vec::new();
    }
    let mut out = Vec::new();
    pull_one(expr, classes, modules, helpers, seq, value_used, &mut out);
    out
}

/// True when some call or write is evaluated before an `extend` in this
/// expression. The extend's own receiver does not count: it is moved
/// into the setup, immediately before the extend.
fn effect_precedes_extend(expr: &Expr) -> bool {
    let mut effect = false;
    let mut preceded = false;
    scan_eval_order(expr, &mut effect, &mut preceded);
    preceded
}

fn scan_eval_order(expr: &Expr, effect: &mut bool, preceded: &mut bool) {
    if *preceded || matches!(&*expr.node, ExprNode::Lambda { .. }) {
        return;
    }
    // A statement sequence already runs in order. An earlier statement
    // is not a reason to refuse an extend in a later one; only an
    // effect inside the same expression is.
    if let ExprNode::Seq { exprs } = &*expr.node {
        for child in exprs {
            let mut local_effect = false;
            scan_eval_order(child, &mut local_effect, preceded);
        }
        return;
    }
    if gates_control(expr) {
        *effect = true;
        return;
    }
    // `obj.n += expr` reads `obj.n` before `expr`. That read is not a
    // child node, so an extend in the right-hand side is preceded.
    if let ExprNode::OpAssign { value, .. } = &*expr.node {
        *effect = true;
        scan_eval_order(value, effect, preceded);
        return;
    }
    if instance_extend(expr).is_some() {
        if *effect {
            *preceded = true;
        }
        *effect = true;
        return;
    }
    expr.node.for_each_child(&mut |child| {
        if !*preceded {
            scan_eval_order(child, effect, preceded);
        }
    });
    if is_effect(expr) {
        *effect = true;
    }
}

fn is_effect(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Send { .. }
            | ExprNode::Assign { .. }
            | ExprNode::OpAssign { .. }
            | ExprNode::MultiAssign { .. }
            | ExprNode::Yield { .. }
            | ExprNode::Raise { .. }
            | ExprNode::Super { .. }
            | ExprNode::Return { .. }
    )
}

fn pull_one(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
    out: &mut Vec<Expr>,
) {
    if matches!(&*expr.node, ExprNode::Lambda { .. }) || gates_control(expr) {
        return;
    }
    if let Some(setups) = explode_extend(expr, classes, modules, helpers, seq, value_used) {
        out.extend(setups);
    }
    expr.node.for_each_child_mut(&mut |c| {
        pull_one(c, classes, modules, helpers, seq, true, out);
    });
}

/// `if` / `&&` / `||` (and the other short-circuit forms) run only some
/// of their children. Setups have to stay in that arm. Pulling them out
/// to the enclosing statement would extend the object when the branch
/// did not run, so an `extend` written there is left in place and the
/// test is stubbed.
fn gates_control(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::If { .. }
        | ExprNode::BoolOp { .. }
        | ExprNode::Case { .. }
        | ExprNode::CaseMatch { .. }
        | ExprNode::While { .. }
        | ExprNode::RescueModifier { .. }
        | ExprNode::BeginRescue { .. } => true,
        ExprNode::OpAssign { op, .. } => {
            matches!(
                op,
                crate::expr::OpAssignOp::OrOr | crate::expr::OpAssignOp::AndAnd
            )
        }
        _ => false,
    }
}

struct Built {
    new_call: Expr,
    sets: Vec<Expr>,
}

/// Replace `extend`'s receiver with a fresh local and return the
/// statements that bind it, when the receiver is one `.new` plus
/// setter calls on that object.
///
/// Spinel flips the object's class at the `extend` *statement* and the
/// expression itself does not answer the object. A use of the value
/// (`request(obj.extend(mod))`, `obj.extend(mod).ping`) becomes the
/// statement followed by the local.
fn explode_extend(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
) -> Option<Vec<Expr>> {
    let span = expr.span;
    let extend_of_known_module = matches!(&*expr.node, ExprNode::Send { recv: Some(_), method, args, .. }
        if method.as_str() == "extend" && args.len() == 1 && const_name(&args[0]).is_some_and(|n| modules.contains(&n)));
    if !extend_of_known_module {
        return None;
    }
    let receiver_is_local =
        matches!(&*expr.node, ExprNode::Send { recv: Some(recv), .. } if var_name(recv).is_some());
    if receiver_is_local {
        if !value_used {
            return None;
        }
        let stmt = expr.clone();
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &*stmt.node
        else {
            return None;
        };
        let local_expr = recv.clone();
        *expr = local_expr;
        return Some(vec![stmt]);
    }
    let built = {
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &*expr.node
        else {
            return None;
        };
        explode_receiver(recv, helpers)?
    };
    let class_name = plain_new_class(&built.new_call)?;
    if !classes.contains(&class_name) {
        return None;
    }
    *seq += 1;
    let local = Symbol::from(format!("__rh_extend_object_{seq}"));
    let mut setups = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: local.clone(),
            },
            value: built.new_call,
        },
    )];
    let local_expr = Expr::new(
        span,
        ExprNode::Var {
            id: VarId(0),
            name: local.clone(),
        },
    );
    for mut set in built.sets {
        subst_name(&mut set, &receiver_placeholder(), &local_expr);
        setups.push(set);
    }
    {
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &mut *expr.node
        else {
            return None;
        };
        *recv = local_expr.clone();
    }
    if value_used {
        let stmt = expr.clone();
        *expr = local_expr;
        setups.push(stmt);
    }
    Some(setups)
}

fn explode_receiver(recv: &Expr, helpers: &[Factory]) -> Option<Built> {
    if let Some(built) = explode_expr(recv) {
        return Some(built);
    }
    let ExprNode::Send {
        recv: target,
        method,
        args,
        block: None,
        ..
    } = &*recv.node
    else {
        return None;
    };
    let implicit = match target {
        None => true,
        Some(t) => matches!(&*t.node, ExprNode::SelfRef),
    };
    if !implicit {
        return None;
    }
    let factory = helpers
        .iter()
        .find(|f| f.name.as_str() == method.as_str())?;
    let bound = bind_params(&factory.params, args)?;
    // An argument with a call has to run before `Class.new`. Splicing
    // it into the body runs it at the use, which is later, and a second
    // use runs it twice. A literal, constant, or local can move.
    if bound.iter().any(|(_, value)| !pure_binding(value)) {
        return None;
    }
    let mut body = value_of(&factory.body).clone();
    // Qualify and rename the tap parameter before the caller's
    // arguments are spliced in. Otherwise an argument named `box`, or
    // a bare constant that matches the helper, is rewritten too.
    if !factory.constants.is_empty() {
        qualify(&mut body, &factory.owner, &factory.constants);
    }
    let mut built = explode_expr(&body)?;
    for (name, value) in &bound {
        subst_name(&mut built.new_call, name, value);
        for set in &mut built.sets {
            subst_name(set, name, value);
        }
    }
    Some(built)
}

fn explode_expr(expr: &Expr) -> Option<Built> {
    if plain_new_class(expr).is_some() {
        return Some(Built {
            new_call: expr.clone(),
            sets: Vec::new(),
        });
    }
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        block: Some(block),
        ..
    } = &*expr.node
    else {
        return None;
    };
    if method.as_str() != "tap" || !args.is_empty() || plain_new_class(inner).is_none() {
        return None;
    }
    let ExprNode::Lambda {
        params,
        rest_param,
        extra_params,
        body,
        ..
    } = &*block.node
    else {
        return None;
    };
    if params.len() != 1 || rest_param.is_some() || !extra_params.is_empty() {
        return None;
    }
    let param = params[0].clone();
    let sets = setter_sends(body, &param)?;
    let mut built = Built {
        new_call: (*inner).clone(),
        sets,
    };
    let placeholder = receiver_placeholder();
    for set in &mut built.sets {
        subst_name(
            set,
            &param,
            &Expr::new(
                set.span,
                ExprNode::Var {
                    id: VarId(0),
                    name: placeholder.clone(),
                },
            ),
        );
    }
    Some(built)
}

fn receiver_placeholder() -> Symbol {
    Symbol::from("__rh_extend_receiver")
}

fn setter_sends(body: &Expr, param: &Symbol) -> Option<Vec<Expr>> {
    let mut out = Vec::new();
    for stmt in statements(body) {
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*stmt.node
        else {
            return None;
        };
        if !var_is(recv, param) || args.len() != 1 || !is_setter(method.as_str()) {
            return None;
        }
        out.push(stmt.clone());
    }
    Some(out)
}

fn is_setter(name: &str) -> bool {
    name.ends_with('=')
        && !matches!(name, "==" | "!=" | "<=" | ">=" | "===" | "=~" | "[]=")
        && name.len() > 1
}

fn bind_params(params: &[Param], args: &[Expr]) -> Option<Vec<(Symbol, Expr)>> {
    if params
        .iter()
        .any(|p| p.keyword || p.rest || p.forwarding || p.from_keyword || p.from_kwrest)
    {
        return None;
    }
    let mut bound = Vec::new();
    let mut i = 0;
    for p in params {
        if i < args.len() {
            bound.push((p.name.clone(), args[i].clone()));
            i += 1;
        } else if let Some(default) = &p.default {
            let mut default = default.clone();
            // `note = tag` means the earlier parameter, not a local of
            // the method the body is copied into.
            for (name, value) in &bound {
                subst_name(&mut default, name, value);
            }
            bound.push((p.name.clone(), default));
        } else {
            return None;
        }
    }
    if i != args.len() {
        None
    } else {
        Some(bound)
    }
}

fn pure_binding(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Lit { .. } | ExprNode::Const { .. } | ExprNode::Var { .. } | ExprNode::SelfRef
    )
}

fn value_of(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => value_of(&exprs[0]),
        ExprNode::Return { value } => value_of(value),
        _ => expr,
    }
}

fn subst_name(expr: &mut Expr, name: &Symbol, replacement: &Expr) {
    if var_is(expr, name) {
        let span = expr.span;
        *expr = replacement.clone();
        if expr.span.is_synthetic() {
            expr.span = span;
        }
        return;
    }
    expr.node
        .for_each_child_mut(&mut |c| subst_name(c, name, replacement));
}

fn qualify(expr: &mut Expr, owner: &ClassId, constants: &[Symbol]) {
    if let ExprNode::Const { path } = &mut *expr.node {
        if path.len() == 1 && constants.iter().any(|c| c == &path[0]) {
            let mut full: Vec<Symbol> = owner
                .0
                .as_str()
                .split("::")
                .filter(|s| !s.is_empty())
                .map(Symbol::from)
                .collect();
            full.push(path[0].clone());
            *path = full;
        }
    }
    expr.node
        .for_each_child_mut(&mut |c| qualify(c, owner, constants));
}

fn first_untraced(
    body: &Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    extend_defined: &HashSet<String>,
) -> Option<Diagnostic> {
    let mut found = None;
    walk(body, &mut |e| {
        if found.is_some() {
            return;
        }
        if let Some(d) = untraced_at(e, body, classes, modules, extend_defined) {
            found = Some(d);
        }
    });
    found
}

fn untraced_at(
    expr: &Expr,
    root: &Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    extend_defined: &HashSet<String>,
) -> Option<Diagnostic> {
    if gates_control(expr) || effect_precedes_extend(expr) {
        if let Some(site) =
            extend_outside_a_block(expr).or_else(|| instance_extend(expr).map(|(e, _)| e))
        {
            let mut d = Diagnostic::unsupported(site.span, None, CONSTRUCT, DETAIL);
            d.severity = Severity::Warning;
            return Some(d);
        }
    }
    let (recv, _) = instance_extend(expr)?;
    if traced(recv, expr, root, classes, modules, extend_defined) {
        return None;
    }
    let mut d = Diagnostic::unsupported(expr.span, None, CONSTRUCT, DETAIL);
    d.severity = Severity::Warning;
    Some(d)
}

/// An `extend` in this arm, not one inside a block. A block is its own
/// scope: its setups stay inside it, so they run when the block runs.
fn extend_outside_a_block(expr: &Expr) -> Option<&Expr> {
    let mut found = None;
    expr.node.for_each_child(&mut |child| {
        if found.is_none() {
            found = extend_in_arm(child);
        }
    });
    found
}

fn extend_in_arm(expr: &Expr) -> Option<&Expr> {
    if matches!(&*expr.node, ExprNode::Lambda { .. }) {
        return None;
    }
    if instance_extend(expr).is_some() {
        return Some(expr);
    }
    let mut found = None;
    expr.node.for_each_child(&mut |child| {
        if found.is_none() {
            found = extend_in_arm(child);
        }
    });
    found
}

fn traced(
    recv: &Expr,
    extend_expr: &Expr,
    root: &Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    extend_defined: &HashSet<String>,
) -> bool {
    let ExprNode::Send { args, .. } = &*extend_expr.node else {
        return false;
    };
    // One module. A second argument (`extend(Mod, other)`) is not the
    // call Spinel transplants, and leaving it in the file refuses the link.
    if args.len() != 1 {
        return false;
    }
    let Some(mod_name) = const_name(&args[0]) else {
        return false;
    };
    if !modules.contains(&mod_name) {
        return false;
    }
    let Some(local) = var_name(recv) else {
        return false;
    };
    let mut values = Vec::new();
    let others = std::cell::Cell::new(false);
    visit_scoped(
        root,
        &|e| contains_expr(e, extend_expr),
        &mut |e| {
            if let ExprNode::Assign {
                target: LValue::Var { name, .. },
                value,
            } = &*e.node
            {
                if name == &local {
                    values.push(value.clone());
                }
            } else if is_other_write(e, &local) {
                others.set(true);
            }
        },
        &mut |e| {
            if is_plain_assign_to(e, &local) || is_other_write(e, &local) {
                others.set(true);
            }
        },
    );
    if others.get() || values.len() != 1 {
        return false;
    }
    let Some(class_name) = plain_new_class(&values[0]) else {
        return false;
    };
    // `def extend` on the class is that method. Replacing the call with
    // the receiver drops the value it returns.
    if extend_defined.contains(&class_name) {
        return false;
    }
    classes.contains(&class_name)
}

fn instance_extend(expr: &Expr) -> Option<(&Expr, ())> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        ..
    } = &*expr.node
    else {
        return None;
    };
    if method.as_str() != "extend" {
        return None;
    }
    if matches!(&*recv.node, ExprNode::Const { .. } | ExprNode::SelfRef) {
        return None;
    }
    Some((recv, ()))
}

fn plain_new_class(expr: &Expr) -> Option<String> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        block: None,
        ..
    } = &*expr.node
    else {
        return None;
    };
    if method.as_str() != "new" {
        return None;
    }
    const_name(recv)
}

fn method_from_source(text: &str, owner: &ClassId) -> Option<MethodDef> {
    let result = ruby_prism::parse(text.as_bytes());
    let stmt = result
        .node()
        .as_program_node()?
        .statements()
        .body()
        .iter()
        .next()?;
    let def = stmt.as_def_node()?;
    crate::ingest::library_class::ingest_library_method(&def, owner, "<instance-extend>").ok()
}

fn source_slice(sources: &[SourceFile], span: Span) -> Option<&str> {
    let file = sources.get(span.file.0.checked_sub(1)? as usize)?;
    file.text.get(span.start as usize..span.end as usize)
}

fn statements(expr: &Expr) -> Vec<&Expr> {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![expr],
    }
}

fn walk(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    expr.node.for_each_child(&mut |c| walk(c, f));
}

fn const_name(expr: &Expr) -> Option<String> {
    let ExprNode::Const { path } = &*expr.node else {
        return None;
    };
    if path.is_empty() {
        return None;
    }
    Some(
        path.iter()
            .filter(|s| !s.as_str().is_empty())
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("::"),
    )
}

fn const_is(expr: &Expr, name: &str) -> bool {
    const_name(expr).as_deref() == Some(name)
}

fn var_name(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn var_is(expr: &Expr, name: &Symbol) -> bool {
    matches!(&*expr.node, ExprNode::Var { name: n, .. } if n == name)
}

fn is_plain_assign_to(expr: &Expr, name: &Symbol) -> bool {
    matches!(&*expr.node, ExprNode::Assign { target: LValue::Var { name: n, .. }, .. } if n == name)
}

fn is_other_write(expr: &Expr, name: &Symbol) -> bool {
    match &*expr.node {
        ExprNode::OpAssign {
            target: LValue::Var { name: n, .. },
            ..
        } => n == name,
        ExprNode::MultiAssign { targets, .. } => targets
            .iter()
            .any(|t| matches!(t, LValue::Var { name: n, .. } if n == name)),
        _ => false,
    }
}

fn stub_sites(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| stub_sites(c, diags));
    if instance_extend(expr).is_none() {
        return;
    }
    let mut d = Diagnostic::unsupported(expr.span, None, CONSTRUCT, DETAIL);
    d.severity = Severity::Warning;
    diags.push(d);
    *expr = stub_expr(expr.span);
}

/// A nil carrying the report — the ruby emitter renders it as the raise.
fn stub_expr(span: crate::span::Span) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Nil,
        },
    );
    e.set_diagnostic(Some(DiagnosticKind::Unsupported {
        target: None,
        construct: Symbol::from(CONSTRUCT),
        detail: DETAIL.to_string(),
    }));
    e
}
