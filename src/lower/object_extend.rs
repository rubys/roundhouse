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
use crate::diagnostic::{Diagnostic, DiagnosticKind, NESTED_DEF, Severity};
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, MethodVisibility, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

mod region;
mod rewrite;

const CONSTRUCT: &str = "Object#extend";
const DETAIL: &str = "mixing a module into one live object needs a per-object method \
                      table, which no compiled target has; the site raises";

pub fn apply_object_extend_stub(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| stub_sites(e, &mut diags));
    for view in &mut app.views {
        stub_sites(&mut view.body, &mut diags);
    }
    let library = library_index(app);
    // One closure over library classes and this file's classes. The
    // inner loop used to repeat the parent / include / prepend walk.
    let library_classes = app.library_classes.clone();
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            stub_sites(setup, &mut diags);
        }
        let helpers = factories(tm);
        let mut classes = library.classes.clone();
        let mut base_modules = library.modules.clone();
        for lc in &tm.inner_classes {
            note_class(lc, &library.models, &mut classes, &mut base_modules);
        }
        let extend_defined =
            extend_defined_names(library_classes.iter().chain(tm.inner_classes.iter()));
        let home_constants: Vec<Symbol> = tm.constants.iter().map(|(name, _)| name.clone()).collect();
        for t in &mut tm.tests {
            let mut modules = base_modules.clone();
            let mut absolute = classes.clone();
            absolute.extend(modules.iter().cloned());
            let added = hoist_anonymous_modules(
                &mut t.body,
                &mut tm.inner_classes,
                &mut modules,
                &tm.name,
                &home_constants,
                &absolute,
            );
            let mut seq = 0u32;
            rewrite::rewrite_scope(&mut t.body, &classes, &modules, &helpers, &mut seq);
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
    LibraryIndex {
        models,
        classes,
        modules,
    }
}

fn defines_instance_extend(lc: &LibraryClass) -> bool {
    lc.methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "extend")
}

/// `prepend Mod` is not an `include` entry. The prepended `extend` is
/// still the method the instance calls.
fn prepends_defined_extend(lc: &LibraryClass, defined: &HashSet<String>) -> bool {
    lc.unknown_calls.iter().any(|call| {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } = &*call.node
        else {
            return false;
        };
        method.as_str() == "prepend"
            && args.len() == 1
            && const_name(&args[0]).is_some_and(|name| defined.contains(&name))
    })
}

/// A class, and any class under it, whose `extend` is not `Object#extend`.
fn extend_defined_names<'a>(
    classes: impl IntoIterator<Item = &'a LibraryClass>,
) -> HashSet<String> {
    let classes: Vec<&LibraryClass> = classes.into_iter().collect();
    let mut out = HashSet::new();
    let mut grew = true;
    while grew {
        grew = false;
        for lc in &classes {
            let name = lc.name.0.as_str().to_string();
            if out.contains(&name) {
                continue;
            }
            let parent_defines = lc
                .parent
                .as_ref()
                .is_some_and(|p| out.contains(p.0.as_str()));
            let include_defines = lc.includes.iter().any(|inc| out.contains(inc.0.as_str()));
            if defines_instance_extend(lc)
                || parent_defines
                || include_defines
                || prepends_defined_extend(lc, &out)
            {
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
    /// Class or module whose lexical scope the body was written in.
    owner: ClassId,
    /// Class the body is copied into. Differs from `owner` for an
    /// included module, where a bare constant is not the test class's.
    home: ClassId,
    /// Constants assigned on `owner`.
    constants: Vec<Symbol>,
    /// The method `yield`s, calls `super`, or takes a block. Copying
    /// the body would run that against the test method.
    yields: bool,
}

fn yields_to_caller(method: &MethodDef) -> bool {
    method.block_param.is_some() || method.has_anonymous_block || expr_yields(&method.body)
}

fn expr_yields(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Yield { .. } | ExprNode::Super { .. }) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| {
        if !found {
            found = expr_yields(child);
        }
    });
    found
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
            home: tm.name.clone(),
            constants: Vec::new(),
            yields: yields_to_caller(m),
        });
    }
    // The last `include` wins, as Ruby's ancestor chain does.
    for inc in tm.includes.iter().rev() {
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
                home: tm.name.clone(),
                constants: constants.clone(),
                yields: yields_to_caller(m),
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
    inner: &mut Vec<LibraryClass>,
    modules: &mut HashSet<String>,
    home: &ClassId,
    home_constants: &[Symbol],
    absolute: &HashSet<String>,
) -> usize {
    let mut found = Vec::new();
    collect_module_assigns(body, &mut found);
    let before = inner.len();
    // The survey borrows `body`. The names are taken before the loop
    // rewrites the statements.
    let sole: HashSet<Symbol> = {
        let reads = region::survey(body);
        found
            .iter()
            .filter(|(local, _)| reads.sole_local(local))
            .map(|(local, _)| local.clone())
            .collect()
    };
    let mut n = 0u32;
    for (local, methods) in found {
        if !sole.contains(&local) {
            continue;
        }
        let name = fresh_const(&mut n, |c| {
            inner.iter().any(|lc| lc.name.0.as_str() == c) || modules.contains(c)
        });
        let id = ClassId(Symbol::from(name.as_str()));
        let mut defs = Vec::new();
        let mut anchored = true;
        for (vis, mut def) in methods {
            // The module is emitted at file scope. A bare `LABEL` written
            // in the test class would otherwise resolve at the top level.
            if !anchor_method(&mut def, home, home_constants, absolute) {
                anchored = false;
                break;
            }
            def.visibility = vis;
            def.enclosing_class = Some(id.0.clone());
            defs.push(def);
        }
        if !anchored || defs.is_empty() {
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

fn anchor_method(
    def: &mut MethodDef,
    home: &ClassId,
    constants: &[Symbol],
    absolute: &HashSet<String>,
) -> bool {
    for param in &mut def.params {
        if let Some(default) = &mut param.default {
            if !rewrite::anchor_constants(default, home, constants, absolute) {
                return false;
            }
        }
    }
    if let Some(param) = &mut def.block_param {
        if let Some(default) = &mut param.default {
            if !rewrite::anchor_constants(default, home, constants, absolute) {
                return false;
            }
        }
    }
    rewrite::anchor_constants(&mut def.body, home, constants, absolute)
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
    out: &mut Vec<(Symbol, Vec<(MethodVisibility, MethodDef)>)>,
) {
    collect_module_statements(expr, out);
}

/// Only a statement assignment is hoisted. `foo(failing = Module.new { ... })`
/// uses the assignment's value; replacing it with nil would change `foo`.
fn collect_module_statements(
    expr: &Expr,
    out: &mut Vec<(Symbol, Vec<(MethodVisibility, MethodDef)>)>,
) {
    match &*expr.node {
        ExprNode::Seq { exprs } => {
            for stmt in exprs {
                collect_one_statement(stmt, out);
            }
        }
        _ => collect_one_statement(expr, out),
    }
}

fn collect_one_statement(expr: &Expr, out: &mut Vec<(Symbol, Vec<(MethodVisibility, MethodDef)>)>) {
    if let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &*expr.node
    {
        if let Some(methods) = module_new_methods(value) {
            out.push((name.clone(), methods));
        }
    }
    if let ExprNode::Lambda { body, .. } = &*expr.node {
        collect_module_statements(body, out);
        return;
    }
    if let ExprNode::Send { block, .. } = &*expr.node {
        if let Some(block) = block {
            collect_module_statements(block, out);
        }
    }
}

fn module_new_methods(expr: &Expr) -> Option<Vec<(MethodVisibility, MethodDef)>> {
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
                out.push((v, args[0].nested_def()?.clone()));
            }
            _ if is_nested_def(stmt) => out.push((vis, stmt.nested_def()?.clone())),
            _ => return None,
        }
    }
    if out.is_empty() { None } else { Some(out) }
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

fn first_untraced(
    body: &Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    extend_defined: &HashSet<String>,
) -> Option<Diagnostic> {
    let survey = region::survey(body);
    for site in survey.instance_extends() {
        if site_supported(&site, classes, modules, extend_defined) {
            continue;
        }
        let mut d = Diagnostic::unsupported(site.expr.span, None, CONSTRUCT, DETAIL);
        d.severity = Severity::Warning;
        return Some(d);
    }
    None
}

/// Spinel flips the class at one `extend` statement whose receiver is
/// the single straight-line `.new` and whose argument is one module.
fn site_supported(
    site: &region::ExtendSite<'_>,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    extend_defined: &HashSet<String>,
) -> bool {
    if site.delayed || !site.is_stmt || site.preceded || site.gated {
        return false;
    }
    let ExprNode::Send { args, .. } = &*site.expr.node else {
        return false;
    };
    // A second argument (`extend(Mod, other)`) is not the call Spinel
    // transplants, and leaving it in the file refuses the link.
    if args.len() != 1 {
        return false;
    }
    let Some(mod_name) = const_name(&args[0]) else {
        return false;
    };
    if !modules.contains(&mod_name) {
        return false;
    }
    let Some(class_name) = site.reaching_class.as_deref() else {
        return false;
    };
    // `def extend` on the class is that method. Replacing the call with
    // the receiver drops the value it returns.
    !extend_defined.contains(class_name) && classes.contains(class_name)
}

/// `if` / `&&` / `||` (and the other short-circuit forms) run only some
/// of their children. A write there is not the one straight-line definition.
pub(super) fn gates_control(expr: &Expr) -> bool {
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

fn statements(expr: &Expr) -> Vec<&Expr> {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![expr],
    }
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
